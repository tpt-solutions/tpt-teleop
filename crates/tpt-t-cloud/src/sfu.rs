//! WebRTC SFU media routing over lock-free SPSC rings.
//!
//! The roadmap names `webrtc-rs` for this; its tree pulls `tokio` plus
//! Apache-2.0-only branches and is banned by the workspace §2 MIT chain (see
//! `deny.toml`), exactly as `quinn` was in Phase 7. This module meets the
//! same contract — selective fan-out of decoded media/telemetry frames from a
//! single publisher to many subscribers — with the in-house
//! [`tpt_t_ring`] wait-free SPSC ring: each subscriber owns its own ring, the
//! publisher pushes once per subscriber, and congestion drops frames instead
//! of allocating or blocking (consistent with the §6 zero-alloc hot path).
//!
//! Actual WebRTC SDP/DTLS negotiation is deferred to hardware/RTS bring-up and
//! fails loudly (see [`negotiate_webrtc_stub`]); once negotiated externally,
//! the decoded media frames are fed here for routing.

use std::collections::HashMap;
use std::sync::{Arc, Weak};

use tpt_t_ring::SpscRing;

use tpt_t_link::mux::MAX_PAYLOAD;

/// Largest single media frame this router will carry (matches the link MTU
/// payload ceiling so a received datagram routes verbatim).
pub const MAX_MEDIA: usize = MAX_PAYLOAD;

/// One media/telemetry frame in flight through the SFU.
///
/// Copy-on-publish: the publisher stamps one [`MediaFrame`] and pushes it into
/// each subscriber ring. `Copy` + plain-old-data keeps the hot path allocation
/// free; the trailing `payload` is a fixed inline buffer (zero hot-path
/// growth).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub struct MediaFrame {
    /// Channel discriminant (control/telemetry/media/ice…).
    pub channel: u8,
    /// Frame sequence number.
    pub seq: u64,
    /// Valid byte count in [`MediaFrame::payload`].
    pub len: u16,
    /// Inline payload bytes (valid prefix is `len`).
    pub payload: [u8; MAX_MEDIA],
}

impl MediaFrame {
    /// Builds a frame from a payload slice (truncated to [`MAX_MEDIA`]).
    pub fn new(channel: u8, seq: u64, payload: &[u8]) -> Self {
        let mut f = MediaFrame {
            channel,
            seq,
            len: 0,
            payload: [0u8; MAX_MEDIA],
        };
        let n = payload.len().min(MAX_MEDIA);
        f.payload[..n].copy_from_slice(&payload[..n]);
        f.len = n as u16;
        f
    }

    /// The valid payload slice.
    pub fn bytes(&self) -> &[u8] {
        &self.payload[..self.len as usize]
    }
}

/// Stable subscriber identity handed back to callers.
pub type SubscriberId = u64;

/// Selective fan-out router: one publisher per [`SfuFanout`], N subscribers.
///
/// Holding only `Weak` references to subscriber rings lets a subscriber simply
/// drop its `Arc` to leave the session — dead rings are pruned on the next
/// [`publish`](Self::publish). This keeps the router free of explicit
/// teardown handshakes.
#[derive(Default)]
pub struct SfuFanout {
    next: SubscriberId,
    subs: HashMap<SubscriberId, Weak<SpscRing<MediaFrame>>>,
}

impl SfuFanout {
    /// An empty fan-out.
    pub fn new() -> Self {
        Self {
            next: 1,
            subs: HashMap::new(),
        }
    }

    /// Registers a subscriber and returns its id plus the ring it should pop.
    ///
    /// `capacity` is rounded up to a power of two by the ring; it bounds how
    /// far a slow consumer may lag before the publisher's `publish` starts
    /// dropping for that subscriber (congestion isolation — one slow viewer
    /// never stalls the publisher or other viewers).
    pub fn subscribe(&mut self, capacity: usize) -> (SubscriberId, Arc<SpscRing<MediaFrame>>) {
        let id = self.next;
        self.next = self.next.wrapping_add(1);
        let ring = Arc::new(SpscRing::with_capacity(capacity.max(1)));
        self.subs.insert(id, Arc::downgrade(&ring));
        (id, ring)
    }

    /// Removes a subscriber explicitly.
    pub fn unsubscribe(&mut self, id: SubscriberId) {
        self.subs.remove(&id);
    }

    /// Current live subscriber count.
    pub fn subscriber_count(&self) -> usize {
        self.subs.values().filter(|w| w.strong_count() > 0).count()
    }

    /// Publishes `frame` to every live subscriber. Returns the number of
    /// subscribers that accepted it (a dropped frame counts against delivery).
    pub fn publish(&mut self, frame: MediaFrame) -> usize {
        let mut delivered = 0usize;
        let mut dead = Vec::new();
        for (id, weak) in &self.subs {
            match weak.upgrade() {
                Some(ring) => {
                    if ring.push(frame).is_ok() {
                        delivered += 1;
                    }
                }
                None => dead.push(*id),
            }
        }
        for d in dead {
            self.subs.remove(&d);
        }
        delivered
    }
}

/// WebRTC SDP/DTLS negotiation hook.
///
/// Exists for API-completeness; the actual negotiation requires the
/// `webrtc-rs` stack which is banned by the §2 MIT chain. The in-house
/// [`SfuFanout`] routes already-decoded media frames over lock-free rings, so
/// this only needs to run once peers are negotiated by an external component.
pub fn negotiate_webrtc_stub() -> Result<(), crate::error::CloudError> {
    Err(crate::error::CloudError::Unsupported(
        "WebRTC SDP/DTLS negotiation requires the webrtc-rs stack, which is banned by the \
         workspace §2 MIT chain; in-house SFU routing via SfuFanout is available once peers \
         are negotiated externally",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_frame_carries_payload() {
        let f = MediaFrame::new(3, 17, b"frame-data");
        assert_eq!(f.channel, 3);
        assert_eq!(f.seq, 17);
        assert_eq!(f.bytes(), b"frame-data");
    }

    #[test]
    fn publish_fans_out_to_live_subscribers() {
        let mut sfu = SfuFanout::new();
        let (_, a) = sfu.subscribe(16);
        let (_, b) = sfu.subscribe(16);
        let delivered = sfu.publish(MediaFrame::new(2, 5, b"hello"));
        assert_eq!(delivered, 2);
        assert_eq!(a.pop().unwrap().bytes(), b"hello");
        assert_eq!(b.pop().unwrap().bytes(), b"hello");
        assert_eq!(sfu.subscriber_count(), 2);
    }

    #[test]
    fn dropped_subscriber_is_pruned() {
        let mut sfu = SfuFanout::new();
        let (id, a) = sfu.subscribe(8);
        let _ = sfu.publish(MediaFrame::new(1, 1, b"x"));
        assert_eq!(sfu.subscriber_count(), 1);
        drop(a);
        // Ring still exists (we held a strong ref via `a` which is now dropped,
        // but the map holds only a Weak); upgrade fails → pruned on next publish.
        assert_eq!(sfu.publish(MediaFrame::new(1, 2, b"y")), 0);
        assert_eq!(sfu.subscriber_count(), 0);
        sfu.unsubscribe(id);
    }

    #[test]
    fn congestion_drops_for_full_ring_only() {
        let mut sfu = SfuFanout::new();
        let (_, a) = sfu.subscribe(2); // tiny ring
        assert_eq!(sfu.publish(MediaFrame::new(1, 1, b"1")), 1);
        assert_eq!(sfu.publish(MediaFrame::new(1, 2, b"2")), 1);
        // Ring full: further publish is dropped for this subscriber.
        assert_eq!(sfu.publish(MediaFrame::new(1, 3, b"3")), 0);
        assert_eq!(a.len(), 2);
    }

    #[test]
    fn negotiation_stub_fails_loudly() {
        assert!(negotiate_webrtc_stub().is_err());
    }
}
