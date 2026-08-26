//! Lock-free shared/co-pilot control state (spec §5.1 multi-operator).
//!
//! Up to [`MAX_OPERATORS`] operators hold heartbeats; the hub arbitrates a
//! single effective operator (lowest live id wins) whose commands carry full
//! authority. All state lives in atomics — any thread may heartbeat or
//! query without locks.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// Maximum simultaneous operator slots.
pub const MAX_OPERATORS: usize = 2;

/// Multi-operator arbitration hub.
///
/// Slots are indexed by operator id (0 = chief, 1 = co-pilot). An operator
/// is *live* while its last heartbeat is younger than `timeout_ns`; the
/// lowest-id live operator owns control.
#[derive(Debug)]
pub struct CoPilotHub {
    last_seen: [AtomicU64; MAX_OPERATORS],
    released: [AtomicBool; MAX_OPERATORS],
    timeout_ns: u64,
}

impl CoPilotHub {
    /// Hub with the given heartbeat expiry.
    pub fn new(timeout_ns: u64) -> Self {
        Self {
            last_seen: [AtomicU64::new(0), AtomicU64::new(0)],
            released: [AtomicBool::new(true), AtomicBool::new(true)],
            timeout_ns,
        }
    }

    /// Records a heartbeat: makes slot `id` live until `now + timeout`.
    pub fn heartbeat(&self, id: usize, now_ns: u64) {
        if id >= MAX_OPERATORS {
            return;
        }
        self.last_seen[id].store(now_ns, Ordering::Release);
        self.released[id].store(false, Ordering::Release);
    }

    /// Explicitly yields control before the timeout expires.
    pub fn release(&self, id: usize) {
        if id < MAX_OPERATORS {
            self.released[id].store(true, Ordering::Release);
        }
    }

    fn live(&self, id: usize, now_ns: u64) -> bool {
        !self.released[id].load(Ordering::Acquire)
            && now_ns.saturating_sub(self.last_seen[id].load(Ordering::Acquire)) <= self.timeout_ns
    }

    /// The operator currently owning control, if any.
    pub fn effective_operator(&self, now_ns: u64) -> Option<usize> {
        (0..MAX_OPERATORS).find(|&id| self.live(id, now_ns))
    }

    /// Authority weight for `id` given the current owner: 1.0 when owning,
    /// otherwise 0.0 (a silent co-pilot cannot inject partial input).
    pub fn authority_for(&self, id: usize, now_ns: u64) -> f32 {
        match self.effective_operator(now_ns) {
            Some(owner) if owner == id => 1.0,
            _ => 0.0,
        }
    }
}
