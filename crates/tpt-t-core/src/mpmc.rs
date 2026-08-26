//! Bounded lock-free MPMC queue (Vyukov's bounded queue, adapted).
//!
//! Unlike the SPSC rings in tpt-t-ring, this structure allows any
//! number of producers and consumers. It is *lock-free* (a stalled thread
//! cannot block others indefinitely) but not wait-free: contending CAS
//! retries are bounded in practice by hardware fairness. Use SPSC rings on
//! the deterministic hot path; use this for control-plane fan-in/fan-out
//! where multiple threads contend (bus free-lists, pool recycling).
//!
//! Cells carry a sequence number each; `enqueue` claims via CAS on
//! `enqueue_pos`, `dequeue` via CAS on `dequeue_pos`. Monotonic positions +
//! wrapping arithmetic give correct wrap-around for the full `usize` range.

use core::cell::UnsafeCell;
use core::fmt;
use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicUsize, Ordering};

use tpt_t_ring::CachePadded;

struct Cell<T> {
    seq: UnsafeCell<AtomicUsize>,
    value: UnsafeCell<MaybeUninit<T>>,
}

/// Bounded MPMC queue. `capacity` rounds up to a power of two.
pub struct MpmcRing<T> {
    cells: Box<[Cell<T>]>,
    enqueue_pos: CachePadded<AtomicUsize>,
    dequeue_pos: CachePadded<AtomicUsize>,
    cap: usize,
}

// SAFETY: cell ownership is handed over exclusively via CAS'd position
// counters: exactly one producer owns a cell between claim and publish,
// ditto one consumer. Sound iff T: Send.
unsafe impl<T: Send> Send for MpmcRing<T> {}
// SAFETY: &MpmcRing multiplexes claims through the same CAS loops; cell
// exclusivity holds identically under concurrent access.
unsafe impl<T: Send> Sync for MpmcRing<T> {}

impl<T> MpmcRing<T> {
    /// Creates the queue; capacity rounds up to a power of two (min 1).
    pub fn with_capacity(capacity: usize) -> Self {
        let cap = capacity.max(1).next_power_of_two();
        let cells: Box<[Cell<T>]> = (0..cap)
            .map(|i| Cell {
                seq: UnsafeCell::new(AtomicUsize::new(i)),
                value: UnsafeCell::new(MaybeUninit::uninit()),
            })
            .collect();
        Self {
            cells,
            enqueue_pos: CachePadded(AtomicUsize::new(0)),
            dequeue_pos: CachePadded(AtomicUsize::new(0)),
            cap,
        }
    }

    /// Maximum in-flight elements (power of two).
    pub fn capacity(&self) -> usize {
        self.cap
    }

    /// Enqueues `value`; `Err(value)` when full. Lock-free.
    pub fn push(&self, value: T) -> Result<(), T> {
        let mut pos = self.enqueue_pos.0.load(Ordering::Relaxed);
        loop {
            // SAFETY: pos & mask indexes an allocated cell; exclusive write
            // access proven by the seq protocol below.
            let cell = unsafe { self.cells.get_unchecked(pos & (self.cap - 1)) };
            // SAFETY: single-threaded-per-cell access per seq protocol.
            let seq = unsafe { &*cell.seq.get() };
            let diff = (seq.load(Ordering::Acquire).wrapping_sub(pos)) as isize;
            if diff == 0 {
                match self.enqueue_pos.0.compare_exchange_weak(
                    pos,
                    pos.wrapping_add(1),
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => {
                        // SAFETY: we won exclusive right to fill this cell.
                        unsafe { (*cell.value.get()).write(value) };
                        seq.store(pos.wrapping_add(1), Ordering::Release);
                        return Ok(());
                    }
                    Err(p) => pos = p, // lost race: retry on latest position
                }
            } else if diff < 0 {
                return Err(value); // full
            } else {
                pos = self.enqueue_pos.0.load(Ordering::Relaxed); // stale view
            }
        }
    }

    /// Dequeues the oldest element; `None` when empty. Lock-free.
    pub fn pop(&self) -> Option<T> {
        let mut pos = self.dequeue_pos.0.load(Ordering::Relaxed);
        loop {
            // SAFETY: see push().
            let cell = unsafe { self.cells.get_unchecked(pos & (self.cap - 1)) };
            // SAFETY: seq protocol grants unique read during our claim.
            let seq = unsafe { &*cell.seq.get() };
            let diff = (seq
                .load(Ordering::Acquire)
                .wrapping_sub(pos.wrapping_add(1))) as isize;
            if diff == 0 {
                match self.dequeue_pos.0.compare_exchange_weak(
                    pos,
                    pos.wrapping_add(1),
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => {
                        // SAFETY: published by producer's Release store above.
                        let v = unsafe { (*cell.value.get()).assume_init_read() };
                        seq.store(pos.wrapping_add(self.cap), Ordering::Release);
                        return Some(v);
                    }
                    Err(p) => pos = p,
                }
            } else if diff < 0 {
                return None; // empty
            } else {
                pos = self.dequeue_pos.0.load(Ordering::Relaxed);
            }
        }
    }

    /// Approximate element count (racy under contention).
    pub fn len(&self) -> usize {
        self.enqueue_pos
            .0
            .load(Ordering::Acquire)
            .wrapping_sub(self.dequeue_pos.0.load(Ordering::Acquire))
    }

    /// True when no elements pending.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl<T> Drop for MpmcRing<T> {
    fn drop(&mut self) {
        while self.pop().is_some() {}
    }
}

impl<T> fmt::Debug for MpmcRing<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MpmcRing")
            .field("capacity", &self.cap)
            .field("len", &self.len())
            .finish()
    }
}
