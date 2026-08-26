//! Reliable control channel — the QUIC fallback slot (spec §5.2).
//!
//! The spec names `quinn`, but quinn cannot ship under this workspace's §2
//! MIT chain: its tree hard-depends on tokio (an async runtime the
//! architecture forbids everywhere) and resolves rustls branches that are
//! Apache-2.0-only (`aws-lc-rs`). This module therefore provides the same
//! API contract — ordered, duplicate-free, loss-recovering delivery of small
//! control payloads over the raw-UDP multiplexer — as an in-house
//! selective-repeat protocol. A quinn transport may still be swapped in
//! behind this API if dependency policy ever changes.
//!
//! Protocol shape (v1):
//!
//! * Sender retains each frame until acknowledged; sequence numbers come
//!   from [`tpt_t_core::ser::ControlCommand::seq`] so no extra wire field is
//!   needed.
//! * Receiver reports cumulative `base_seq` plus a 32-bit SACK bitmap of
//!   out-of-order arrivals in an [`AckFrame`].
//! * Unacknowledged frames are retransmitted on an RTO timer
//!   ([`ReliableTx::tick`]).
//! * RTT samples flow back into the link's backpressure signal.
//!
//! Fixed-capacity retention means **no allocation** anywhere on the path.

use core::fmt;
use tpt_t_ring::cast::{bytes_of, ref_from_bytes};

/// Largest retained/rebuffered payload per frame.
pub const MAX_RELIABLE_PAYLOAD: usize = 256;

/// SACK window width (bits in [`AckFrame::bitmap`]).
pub const SACK_BITS: u32 = 32;

/// Highest SACKable offset (pattern-friendly const).
const SACK_MAX: u32 = SACK_BITS - 1;

/// Wire form of one acknowledgement — 16 bytes dense, no padding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub struct AckFrame {
    /// All sequences `< base_seq` are cumulatively acknowledged.
    pub base_seq: u32,
    /// Bit `i` set ⇒ sequence `base_seq + 1 + i` arrived out of order.
    pub bitmap: u32,
    /// Receiver-side RTT sample feeding backpressure (ns; 0 = unknown).
    pub rtt_sample_ns: u64,
}

// SAFETY: repr(C) dense primitives only; density asserted by test below.
unsafe impl tpt_t_ring::cast::PlainBytes for AckFrame {}

impl AckFrame {
    /// Encodes into `out`; returns bytes written (16).
    pub fn encode_into(&self, out: &mut [u8]) -> usize {
        let b = bytes_of(self);
        out[..b.len()].copy_from_slice(b);
        b.len()
    }

    /// Decodes an ack from raw payload bytes.
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        ref_from_bytes::<Self>(bytes).copied().ok()
    }

    /// True if `seq` is covered by this ack (cumulative point or SACKed).
    #[inline]
    pub fn covers(&self, seq: u32) -> bool {
        let diff = seq.wrapping_sub(self.base_seq);
        if diff > u32::MAX / 2 {
            return true; // wrapped far below base ⇒ cumulatively acknowledged
        }
        match diff {
            0 => true,
            1..=SACK_MAX => self.bitmap & (1 << (diff - 1)) != 0,
            _ => false,
        }
    }
}

/// Sender configuration.
#[derive(Debug, Clone, Copy)]
pub struct ReliableConfig {
    /// Retention window (power of two enforced at construction).
    pub window: usize,
    /// Retransmission timeout.
    pub rto_ns: u64,
    /// Give up counting resends beyond this (frames stay queued; the link
    /// layer treats a dead peer as a safety event).
    pub max_resends: u32,
}

impl Default for ReliableConfig {
    fn default() -> Self {
        Self {
            window: 128,
            rto_ns: 10_000_000,      // 10 ms — control-loop friendly
            max_resends: 200,
        }
    }
}

/// Error returned when every retention slot inside the usable window is
/// occupied — the peer has stopped acking or the window is genuinely small.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TxFull(pub u32);

impl fmt::Display for TxFull {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "reliable-tx window exhausted at seq {}", self.0)
    }
}

impl std::error::Error for TxFull {}

struct TxSlot {
    seq: u32, // valid iff `seq == expected index value`
    len: usize,
    bytes: [u8; MAX_RELIABLE_PAYLOAD],
    first_ns: u64,
    last_ns: u64,
    resends: u32,
}

/// Selective-repeat sender with fixed retention.
pub struct ReliableTx {
    slots: Box<[TxSlot]>,
    mask: usize,
    oldest: u32, // lowest possibly-unacked seq
}

impl ReliableTx {
    /// Creates a sender; `window` is rounded up to a power of two (≥ 1).
    pub fn new(cfg: ReliableConfig) -> Self {
        let window = cfg.window.next_power_of_two().max(1);
        Self {
            slots: (0..window)
                .map(|i| TxSlot {
                    seq: i as u32,
                    len: 0,
                    bytes: [0u8; MAX_RELIABLE_PAYLOAD],
                    first_ns: 0,
                    last_ns: 0,
                    resends: 0,
                })
                .collect(),
            mask: window - 1,
            oldest: 0,
        }
    }

    /// Window size in frames.
    #[inline]
    pub fn window(&self) -> usize {
        self.slots.len()
    }

    /// Frames currently retained (unacked).
    pub fn in_flight(&self) -> usize {
        // Slots hold exactly the range [oldest, oldest + window): a slot is
        // live iff its stored len > 0 and it was written for the current lap.
        self.slots
            .iter()
            .filter(|s| s.len > 0 && s.seq.wrapping_sub(self.oldest) < self.slots.len() as u32)
            .count()
    }

    /// Retains `payload` under `seq` for transmission/retransmission.
    pub fn send(&mut self, seq: u32, payload: &[u8], now_ns: u64) -> Result<(), TxFull> {
        assert!(payload.len() <= MAX_RELIABLE_PAYLOAD);
        if seq.wrapping_sub(self.oldest) as usize >= self.slots.len() {
            return Err(TxFull(seq)); // would overwrite unacked history
        }
        let slot = &mut self.slots[(seq as usize) & self.mask];
        if slot.len > 0 && slot.seq == seq && slot.first_ns != 0 {
            return Err(TxFull(seq)); // already queued and not yet acked
        }
        slot.seq = seq;
        slot.bytes[..payload.len()].copy_from_slice(payload);
        slot.len = payload.len();
        slot.first_ns = now_ns;
        slot.last_ns = now_ns;
        slot.resends = 0;
        Ok(())
    }

    /// Applies an ack; returns an RTT sample (ns) when any frame freed by it
    /// carries one (oldest freed frame wins). Karn-style ambiguity from
    /// retransmits is accepted in v1 and documented above.
    pub fn on_ack(&mut self, ack: &AckFrame, now_ns: u64) -> Option<u64> {
        let mut rtt = None;
        // Free cumulatively: advance `oldest` toward base_seq.
        loop {
            let diff = ack.base_seq.wrapping_sub(self.oldest);
            if diff == 0 || diff >= self.slots.len() as u32 {
                break;
            }
            let slot = &mut self.slots[(self.oldest as usize) & self.mask];
            if rtt.is_none() && slot.len > 0 && slot.first_ns != 0 {
                rtt = Some(now_ns.saturating_sub(slot.first_ns));
            }
            slot.len = 0;
            slot.first_ns = 0;
            self.oldest = self.oldest.wrapping_add(1);
        }
        // Free SACKed stragglers inside the window.
        for i in 0..SACK_BITS - 1 {
            if ack.bitmap & (1 << i) != 0 {
                let seq = ack.base_seq.wrapping_add(1 + i);
                if seq.wrapping_sub(self.oldest) < self.slots.len() as u32 {
                    let slot = &mut self.slots[(seq as usize) & self.mask];
                    if slot.seq == seq && slot.len > 0 {
                        if rtt.is_none() && slot.first_ns != 0 {
                            rtt = Some(now_ns.saturating_sub(slot.first_ns));
                        }
                        slot.len = 0;
                        slot.first_ns = 0;
                    }
                }
            }
        }
        rtt
    }

    /// Retransmits every frame overdue by [`ReliableConfig::rto_ns`],
    /// invoking `out(seq, payload)` per frame. Returns the resend count.
    pub fn tick(&mut self, now_ns: u64, rto_ns: u64, out: &mut dyn FnMut(u32, &[u8])) -> usize {
        let mut sent = 0;
        for slot in self.slots.iter_mut() {
            if slot.len == 0 || slot.first_ns == 0 {
                continue;
            }
            if now_ns.saturating_sub(slot.last_ns) >= rto_ns {
                slot.last_ns = now_ns;
                slot.resends += 1;
                sent += 1;
                out(slot.seq, &slot.bytes[..slot.len]);
            }
        }
        sent
    }

    /// Highest resend count among live frames (peer-health probe).
    pub fn max_resends(&self) -> u32 {
        self.slots
            .iter()
            .filter(|s| s.len > 0)
            .map(|s| s.resends)
            .max()
            .unwrap_or(0)
    }
}

/// What [`ReliableRx::accept`] decided about a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Accept {
    /// Next expected sequence — deliver immediately.
    InOrder,
    /// Stored for in-order delivery once the gap fills.
    Buffered,
    /// Already seen or older than the cumulative point — drop.
    Duplicate,
    /// Too far ahead of the expected sequence to buffer (bitmap exhausted).
    Overflow,
}

struct RxSlot {
    len: usize,
    bytes: [u8; MAX_RELIABLE_PAYLOAD],
}

/// Selective-repeat receiver with a fixed reorder buffer
/// ([`SACK_BITS`] slots ahead of the expected sequence).
pub struct ReliableRx {
    next: u32,
    ahead: u32, // bit i ⇒ seq = next + 1 + i is buffered
    slots: Box<[RxSlot]>,
}

impl Default for ReliableRx {
    fn default() -> Self {
        Self::new()
    }
}

impl ReliableRx {
    /// Creates a receiver expecting sequence 0.
    pub fn new() -> Self {
        Self {
            next: 0,
            ahead: 0,
            slots: (0..SACK_BITS)
                .map(|_| RxSlot {
                    len: 0,
                    bytes: [0u8; MAX_RELIABLE_PAYLOAD],
                })
                .collect(),
        }
    }

    /// Next in-order sequence this receiver expects.
    #[inline]
    pub fn next_expected(&self) -> u32 {
        self.next
    }

    /// Sequences currently held out-of-order.
    pub fn buffered_count(&self) -> u32 {
        self.ahead.count_ones()
    }

    /// Offers one frame. Out-of-order frames are copied into the reorder
    /// buffer; follow with [`drain`](Self::drain) to emit anything that just
    /// became contiguous. The in-order frame itself is *not* copied — the
    /// caller delivers it straight from its own buffer when told [`Accept::InOrder`].
    pub fn accept(&mut self, seq: u32, payload: &[u8]) -> Accept {
        assert!(payload.len() <= MAX_RELIABLE_PAYLOAD);
        let diff = seq.wrapping_sub(self.next);
        if diff > u32::MAX / 2 {
            return Accept::Duplicate; // older than the cumulative point
        }
        match diff {
            0 => {
                self.advance();
                Accept::InOrder
            }
            d if d < SACK_BITS => {
                let bit = 1u32 << (d - 1);
                if self.ahead & bit != 0 {
                    return Accept::Duplicate;
                }
                let slot = &mut self.slots[(d - 1) as usize];
                slot.bytes[..payload.len()].copy_from_slice(payload);
                slot.len = payload.len();
                self.ahead |= bit;
                Accept::Buffered
            }
            _ => Accept::Overflow, // cannot represent in the SACK window
        }
    }

    /// Delivers every frame that became contiguous via `out(seq, payload)`;
    /// returns the count delivered.
    pub fn drain(&mut self, out: &mut dyn FnMut(u32, &[u8])) -> usize {
        let mut delivered = 0;
        while self.ahead & 1 != 0 {
            let slot = &mut self.slots[0];
            let len = slot.len;
            let bytes = slot.bytes;
            slot.len = 0;
            self.ahead &= !1;
            self.advance();
            delivered += 1;
            out(self.next.wrapping_sub(1), &bytes[..len]);
        }
        delivered
    }

    /// Builds the ack describing the current receive state.
    pub fn build_ack(&self, rtt_sample_ns: u64) -> AckFrame {
        AckFrame {
            base_seq: self.next,
            bitmap: self.ahead,
            rtt_sample_ns,
        }
    }

    /// Advances past one delivered in-order frame.
    fn advance(&mut self) {
        self.next = self.next.wrapping_add(1);
        self.ahead >>= 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: u64 = 1_000_000;

    #[cfg(test)]
    impl ReliableRx {
        /// Test hook: fast-forwards the cumulative point without delivery.
        fn advance_for_test(&mut self, n: u32) {
            for _ in 0..n {
                self.advance();
            }
        }
    }

    #[test]
    fn ack_frame_layout_is_dense_and_cast_roundtrips() {
        assert_eq!(std::mem::size_of::<AckFrame>(), 16);
        let a = AckFrame {
            base_seq: 9,
            bitmap: 0b101,
            rtt_sample_ns: 250_000,
        };
        let mut buf = [0u8; 32];
        assert_eq!(a.encode_into(&mut buf), 16);
        assert_eq!(AckFrame::decode(&buf[..16]), Some(a));
    }

    #[test]
    fn ack_covers_cumulative_and_sacked_ranges() {
        let a = AckFrame {
            base_seq: 10,
            bitmap: (1 << 0) | (1 << 2), // 11 and 13
            rtt_sample_ns: 0,
        };
        assert!(a.covers(5)); // cumulative past
        assert!(a.covers(10));
        assert!(a.covers(11) && a.covers(13));
        assert!(!a.covers(12) && !a.covers(14));
        assert!(!a.covers(100));
        // Old sequences (wrapped far below base) are covered too.
        assert!(a.covers(10u32.wrapping_sub(1_000)));
    }

    #[test]
    fn in_order_stream_needs_no_retransmits() {
        let cfg = ReliableConfig::default();
        let mut tx = ReliableTx::new(cfg);
        let mut rx = ReliableRx::new();
        let mut delivered = Vec::new();

        for seq in 0u32..8 {
            tx.send(seq, &[seq as u8; 4], seq as u64 * MS).unwrap();
            if rx.accept(seq, &[seq as u8; 4]) != Accept::InOrder {
                panic!("in-order frame misclassified");
            }
            rx.drain(&mut |s, p| delivered.push((s, p.to_vec())));
        }
        assert_eq!(tx.in_flight(), 8, "nothing acked yet");
        let ack = rx.build_ack(1 * MS);
        tx.on_ack(&ack, 8 * MS);
        assert_eq!(tx.in_flight(), 0);
        assert_eq!(
            delivered.iter().map(|(s, _)| *s).collect::<Vec<_>>(),
            (0u32..8).collect::<Vec<_>>()
        );
    }

    #[test]
    fn reorder_is_buffered_and_drained_in_order() {
        let mut rx = ReliableRx::new();
        assert_eq!(rx.accept(0, b"zero"), Accept::InOrder);
        assert_eq!(rx.accept(2, b"two"), Accept::Buffered);
        assert_eq!(rx.accept(3, b"three"), Accept::Buffered);

        let ack = rx.build_ack(0);
        assert_eq!(ack.base_seq, 1);
        assert_eq!(ack.bitmap, 0b011); // seqs 2,3

        assert_eq!(rx.buffered_count(), 2);
        assert_eq!(rx.accept(2, b"dup"), Accept::Duplicate);

        // Gap fills → both buffered frames emerge in order.
        assert_eq!(rx.accept(1, b"one"), Accept::InOrder);
        let mut got = Vec::new();
        assert_eq!(rx.drain(&mut |s, p| got.push((s, p.to_vec()))), 2);
        assert_eq!(
            got,
            vec![(2, b"two".to_vec()), (3, b"three".to_vec())]
        );
        assert_eq!(rx.build_ack(0).base_seq, 4);
    }

    #[test]
    fn loss_recovery_roundtrip_with_rto_retransmit() {
        let cfg = ReliableConfig::default();
        let mut tx = ReliableTx::new(cfg);
        let mut rx = ReliableRx::new();
        let now0 = 1_000 * MS;

        for seq in 0u32..4 {
            tx.send(seq, &[b'p', seq as u8], now0).unwrap();
        }
        // Wire drops seq 1 entirely.
        assert_eq!(rx.accept(0, &[b'p', 0]), Accept::InOrder);
        assert_eq!(rx.accept(2, &[b'p', 2]), Accept::Buffered);
        assert_eq!(rx.accept(3, &[b'p', 3]), Accept::Buffered);
        let ack = rx.build_ack(2 * MS);
        assert!(!ack.covers(1));

        // Sender frees 0, 2, 3; keeps 1.
        let rtt = tx.on_ack(&ack, now0 + 2 * MS);
        assert!(rtt.is_some());
        assert_eq!(tx.in_flight(), 1);

        // RTO fires → seq 1 retransmitted.
        let mut resent = Vec::new();
        let n = tx.tick(now0 + 12 * MS, cfg.rto_ns, &mut |s, p| {
            resent.push((s, p.to_vec()))
        });
        assert_eq!((n, tx.max_resends()), (1, 1));

        // Retransmission arrives → full ordered delivery.
        assert_eq!(rx.accept(1, &resent[0].1), Accept::InOrder);
        let mut order = Vec::new();
        rx.drain(&mut |s, _| order.push(s));
        assert_eq!(order, vec![2, 3]);
        assert_eq!(rx.next_expected(), 4);
    }

    #[test]
    fn window_exhaustion_backpressures_until_acks_arrive() {
        let mut tx = ReliableTx::new(ReliableConfig {
            window: 4,
            ..ReliableConfig::default()
        });
        for seq in 0u32..4 {
            assert!(tx.send(seq, b"x", 0).is_ok());
        }
        assert_eq!(tx.send(4, b"x", 0), Err(TxFull(4)));
        // Acking the first two frees room again.
        let ack = AckFrame {
            base_seq: 2,
            bitmap: 0,
            rtt_sample_ns: 0,
        };
        tx.on_ack(&ack, 1 * MS);
        assert!(tx.send(4, b"x", 1 * MS).is_ok());
        assert!(tx.send(6, b"x", 1 * MS).is_err()); // beyond window span
    }

    #[test]
    fn duplicate_and_stale_frames_are_dropped() {
        let mut rx = ReliableRx::new();
        assert_eq!(rx.accept(0, b"a"), Accept::InOrder);
        assert_eq!(rx.accept(0, b"a-dup"), Accept::Duplicate);
        assert_eq!(rx.accept(5, b"far"), Accept::Overflow);
        assert_eq!(rx.accept(7, b"far"), Accept::Overflow);
        // Old sequence from before the cumulative point.
        rx.advance_for_test(10);
        assert_eq!(rx.accept(3, b"ancient"), Accept::Duplicate);
    }
}




