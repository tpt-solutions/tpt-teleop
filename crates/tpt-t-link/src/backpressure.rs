//! Lock-free network backpressure signal (spec §5.2 "Bandwidth Throttling").
//!
//! The network thread observes congestion evidence (blocked sends, queue
//! depth, RTT samples from the reliable channel) and folds it into atomic
//! state; the media encoder thread (Phase 8) reads a suggested bitrate and
//! an admission budget without ever taking a lock or allocating.
//!
//! Single-writer discipline: mutation methods are called from the network
//! thread only; readers on other threads use [`Ordering::Relaxed`] loads and
//! tolerate momentarily stale values by construction.

use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

/// Fixed-point scale for token-bucket arithmetic (`tokens` are stored as
/// bytes << 16 so sub-byte refill rates accumulate losslessly).
const TOKEN_SCALE: u64 = 1 << 16;

/// Discrete congestion levels, monotonic in severity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum Congestion {
    /// Link healthy: full bitrate.
    Normal = 0,
    /// Occasional blocked sends: trim to 70 %.
    Elevated = 1,
    /// Sustained backlog: 45 %.
    High = 2,
    /// Heavy loss/backlog: 25 % — survival mode.
    Critical = 3,
}

impl Congestion {
    /// Fraction of link capacity suggested at this level (basis points).
    pub fn bitrate_fraction_bp(self) -> u64 {
        match self {
            Congestion::Normal => 10_000,
            Congestion::Elevated => 7_000,
            Congestion::High => 4_500,
            Congestion::Critical => 2_500,
        }
    }

    /// Discriminant.
    #[inline]
    pub fn as_u8(self) -> u8 {
        self as u8
    }

    /// Inverse of [`as_u8`](Self::as_u8).
    #[inline]
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Congestion::Normal),
            1 => Some(Congestion::Elevated),
            2 => Some(Congestion::High),
            3 => Some(Congestion::Critical),
            _ => None,
        }
    }
}

/// Atomic congestion estimator + token bucket. Created once at startup.
///
/// All methods are `&self`; interior mutability via atomics only.
pub struct Backpressure {
    capacity_bps: AtomicU64,
    tokens: AtomicU64,
    last_refill_ns: AtomicU64,
    queue_depth: AtomicUsize,
    peak_queue_depth: AtomicUsize,
    blocked_sends: AtomicU64,
    blocked_window: AtomicU64, // blocked sends in the current ~100 ms window
    window_start_ns: AtomicU64,
    dropped_frames: AtomicU64,
    rtt_ewma_ns: AtomicU64,
}

// SAFETY: every field is an atomic; there is no shared non-atomic state.
unsafe impl Send for Backpressure {}
// SAFETY: &Self exposes only atomic accessors.
unsafe impl Sync for Backpressure {}

impl Default for Backpressure {
    fn default() -> Self {
        // 100 Mbit/s placeholder until fleet config supplies a real estimate.
        Self::new(100_000_000)
    }
}

impl core::fmt::Debug for Backpressure {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Backpressure")
            .field("capacity_bps", &self.capacity_bps())
            .field("queue_depth", &self.queue_depth())
            .field("rtt_ewma_ns", &self.rtt_estimate_ns())
            .finish()
    }
}

impl Backpressure {
    /// Creates the signal with an estimated uplink `capacity_bps`.
    pub fn new(capacity_bps: u64) -> Self {
        Self {
            capacity_bps: AtomicU64::new(capacity_bps.max(1)),
            tokens: AtomicU64::new(0),
            last_refill_ns: AtomicU64::new(0),
            queue_depth: AtomicUsize::new(0),
            peak_queue_depth: AtomicUsize::new(0),
            blocked_sends: AtomicU64::new(0),
            blocked_window: AtomicU64::new(0),
            window_start_ns: AtomicU64::new(0),
            dropped_frames: AtomicU64::new(0),
            rtt_ewma_ns: AtomicU64::new(0),
        }
    }

    /// Updates the estimated uplink capacity (fleet policy hook).
    #[inline]
    pub fn set_capacity_bps(&self, bps: u64) {
        self.capacity_bps.store(bps.max(1), Ordering::Relaxed);
    }

    /// Current capacity estimate.
    #[inline]
    pub fn capacity_bps(&self) -> u64 {
        self.capacity_bps.load(Ordering::Relaxed)
    }

    /// Records an RTT sample (ns) from the reliable channel; EWMA α = ¼.
    pub fn note_rtt_ns(&self, sample_ns: u64) {
        let old = self.rtt_ewma_ns.load(Ordering::Relaxed);
        let next = if old == 0 {
            sample_ns
        } else {
            old - old / 4 + sample_ns / 4
        };
        self.rtt_ewma_ns.store(next, Ordering::Relaxed);
    }

    /// Smoothed RTT estimate (ns); `0` before the first sample.
    #[inline]
    pub fn rtt_estimate_ns(&self) -> u64 {
        self.rtt_ewma_ns.load(Ordering::Relaxed)
    }

    /// Network thread reports the OS/socket queue depth hint.
    pub fn set_queue_depth(&self, depth: usize) {
        self.queue_depth.store(depth, Ordering::Relaxed);
        let _ = self.peak_queue_depth.fetch_max(depth, Ordering::Relaxed);
    }

    /// Current queue-depth hint.
    #[inline]
    pub fn queue_depth(&self) -> usize {
        self.queue_depth.load(Ordering::Relaxed)
    }

    /// Peak queue depth seen since startup.
    #[inline]
    pub fn peak_queue_depth(&self) -> usize {
        self.peak_queue_depth.load(Ordering::Relaxed)
    }

    /// A send hit `EWOULDBLOCK` (kernel buffer full).
    pub fn note_send_blocked(&self, now_ns: u64) {
        // Window counter with lazy ~100 ms reset (single writer thread).
        let start = self.window_start_ns.load(Ordering::Relaxed);
        if start == 0 || now_ns.saturating_sub(start) > 100_000_000 {
            self.blocked_window.store(1, Ordering::Relaxed);
            self.window_start_ns.store(now_ns.max(1), Ordering::Relaxed);
        } else {
            let _ = self.blocked_window.fetch_add(1, Ordering::Relaxed);
        }
        let _ = self.blocked_sends.fetch_add(1, Ordering::Relaxed);
    }

    /// Total blocked sends since startup.
    #[inline]
    pub fn blocked_sends(&self) -> u64 {
        self.blocked_sends.load(Ordering::Relaxed)
    }

    /// Blocked sends within the current ~100 ms observation window.
    #[inline]
    pub fn blocked_recently(&self) -> u64 {
        self.blocked_window.load(Ordering::Relaxed)
    }

    /// A frame was dropped (admission refused, oversize, or demux reject).
    #[inline]
    pub fn note_drop(&self) {
        let _ = self.dropped_frames.fetch_add(1, Ordering::Relaxed);
    }

    /// Total drops since startup.
    #[inline]
    pub fn dropped_frames(&self) -> u64 {
        self.dropped_frames.load(Ordering::Relaxed)
    }
}

impl Backpressure {
    /// Credits the token bucket for elapsed time since the last refill.
    /// Called lazily by [`admit`](Self::admit); two atomics, no locks.
    pub fn refill(&self, now_ns: u64) {
        let last = self.last_refill_ns.load(Ordering::Relaxed);
        if now_ns <= last {
            return; // clock went backwards or duplicate tick: nothing to add
        }
        let cap_bps = self.capacity_bps();
        // bytes accrued = cap_bps * dt_ns / 1e9 / 8
        let gained =
            (cap_bps.saturating_mul(now_ns - last) / 8_000_000_000).saturating_mul(TOKEN_SCALE);
        let _ = self.last_refill_ns.compare_exchange(
            last,
            now_ns,
            Ordering::Relaxed,
            Ordering::Relaxed,
        );
        if gained > 0 {
            // Bucket size: one second of link capacity (burst allowance).
            let burst_cap = cap_bps / 8 * TOKEN_SCALE;
            let _ = self.tokens.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |t| {
                Some((t + gained).min(burst_cap.max(TOKEN_SCALE)))
            });
        }
    }

    /// Attempts to spend `bytes` of bandwidth credit at `now_ns`.
    /// Returns `false` (and counts a drop) when the bucket is empty — the
    /// caller should shed non-critical traffic (telemetry/media first).
    pub fn admit(&self, bytes: usize, now_ns: u64) -> bool {
        self.refill(now_ns);
        let want = (bytes as u64).saturating_mul(TOKEN_SCALE);
        match self
            .tokens
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |t| {
                if t >= want {
                    Some(t - want)
                } else {
                    None
                }
            }) {
            Ok(_) => true,
            Err(_) => {
                self.note_drop();
                false
            }
        }
    }

    /// Remaining credit in whole bytes.
    #[inline]
    pub fn available_bytes(&self) -> u64 {
        self.tokens.load(Ordering::Relaxed) / TOKEN_SCALE
    }

    /// Folds queue depth and recent blocked sends into a congestion level.
    pub fn congestion(&self) -> Congestion {
        let depth = self.queue_depth() as u64;
        let blocked = self.blocked_recently().min(64);
        let score = depth.saturating_mul(2).saturating_add(blocked);
        match score {
            0 => Congestion::Normal,
            1..=7 => Congestion::Elevated,
            8..=39 => Congestion::High,
            _ => Congestion::Critical,
        }
    }

    /// Bitrate the encoder should target right now.
    pub fn suggested_bitrate_bps(&self) -> u64 {
        let frac = self.congestion().bitrate_fraction_bp();
        self.capacity_bps() * frac / 10_000
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;

    const MS: u64 = 1_000_000;

    #[test]
    fn refill_accrues_bytes_proportional_to_elapsed_time() {
        let bp = Backpressure::new(8_000_000); // 1 MB/s
        bp.refill(0);
        assert_eq!(bp.available_bytes(), 0);

        bp.refill(500 * MS); // 0.5 s at 1 MB/s = 500 KB
        assert_eq!(bp.available_bytes(), 500_000);

        // Clock going backwards must not mint credit.
        bp.refill(400 * MS);
        assert_eq!(bp.available_bytes(), 500_000);
    }

    #[test]
    fn admit_spends_credit_and_sheds_when_empty() {
        let bp = Backpressure::new(8_000_000); // 1 MB/s
        bp.refill(0);
        bp.refill(1_000 * MS); // exactly 1 MB credited
        assert!(bp.admit(600_000, 1_000 * MS));
        assert!(bp.admit(400_000, 1_000 * MS));
        assert!(!bp.admit(1, 1_000 * MS), "bucket empty");
        assert_eq!(bp.dropped_frames(), 1);
        // Time heals: another second refills 1 MB again.
        assert!(bp.admit(1, 2_000 * MS));
    }

    #[test]
    fn bucket_is_bounded_by_one_second_burst() {
        let bp = Backpressure::new(8_000_000); // 1 MB/s → bucket cap 1 MB
        bp.refill(0);
        bp.refill(60_000 * MS); // far more than the burst cap
        assert!(
            bp.available_bytes() <= 1_048_576,
            "burst cap violated: {}",
            bp.available_bytes()
        );
    }

    #[test]
    fn congestion_tracks_depth_and_blocked_sends_monotonically() {
        let bp = Backpressure::default();
        assert_eq!(bp.congestion(), Congestion::Normal);
        assert_eq!(bp.suggested_bitrate_bps(), 100_000_000);

        bp.set_queue_depth(2);
        bp.note_send_blocked(1);
        assert_eq!(bp.congestion(), Congestion::Elevated);
        assert_eq!(bp.suggested_bitrate_bps(), 70_000_000);

        bp.set_queue_depth(16);
        assert_eq!(bp.congestion(), Congestion::High);
        assert_eq!(bp.suggested_bitrate_bps(), 45_000_000);

        bp.set_queue_depth(64);
        assert_eq!(bp.congestion(), Congestion::Critical);
        assert_eq!(bp.suggested_bitrate_bps(), 25_000_000);
        assert_eq!(
            Congestion::from_u8(bp.congestion().as_u8()),
            Some(bp.congestion())
        );
    }

    #[test]
    fn rtt_ewma_converges_toward_samples() {
        let bp = Backpressure::default();
        bp.note_rtt_ns(1_000);
        assert_eq!(bp.rtt_estimate_ns(), 1_000);
        bp.note_rtt_ns(5_000);
        assert_eq!(bp.rtt_estimate_ns(), 2_000); // 750 + 1250
        for _ in 0..40 {
            bp.note_rtt_ns(5_000);
        }
        assert!((bp.rtt_estimate_ns() as i64 - 5_000).abs() < 100);
    }

    #[test]
    fn shared_across_threads_without_locks() {
        let bp = Arc::new(Backpressure::new(8_000_000));
        let writers: Vec<_> = (0..4)
            .map(|_| {
                let bp = Arc::clone(&bp);
                thread::spawn(move || {
                    for i in 0..1_000u64 {
                        bp.note_rtt_ns(100 + i);
                        bp.set_queue_depth((i % 4) as usize);
                        bp.note_drop();
                    }
                })
            })
            .collect();
        for w in writers {
            w.join().unwrap();
        }
        assert_eq!(bp.dropped_frames(), 4_000);
        assert!(bp.queue_depth() < 4);
    }
}



