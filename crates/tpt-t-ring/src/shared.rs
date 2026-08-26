//! SPSC rings attached to caller-provided memory (shared memory / mmapped).
//!
//! Semantics are identical to [`SpscRing`](crate::SpscRing); only storage
//! differs. The header ([`SharedHeader`]) occupies the first bytes of the
//! region and holds both cache-padded cursors — see [`crate::layout`] for
//! the exact byte math used to size a shm segment.

use core::fmt;
use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicUsize, Ordering};

use crate::cache_line::CACHE_LINE;
use crate::spsc::{do_pop, do_push};

/// The two ring cursors, cache-line isolated, placed at offset 0 of a shared
/// region. Construct once via [`SharedHeader::new`] before attaching.
#[repr(C, align(64))]
pub struct SharedHeader {
    /// Consumer cursor: next index to pop.
    pub head: AtomicUsize,
    _pad_head: [u8; CACHE_LINE - core::mem::size_of::<AtomicUsize>()],
    /// Producer cursor: next index to push.
    pub tail: AtomicUsize,
    _pad_tail: [u8; CACHE_LINE - core::mem::size_of::<AtomicUsize>()],
}

impl SharedHeader {
    /// Zeroed cursors for a fresh region.
    pub const fn new() -> Self {
        Self {
            head: AtomicUsize::new(0),
            _pad_head: [0; CACHE_LINE - core::mem::size_of::<AtomicUsize>()],
            tail: AtomicUsize::new(0),
            _pad_tail: [0; CACHE_LINE - core::mem::size_of::<AtomicUsize>()],
        }
    }
}

impl Default for SharedHeader {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for SharedHeader {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SharedHeader")
            .field("head", &self.head.load(Ordering::Acquire))
            .field("tail", &self.tail.load(Ordering::Acquire))
            .finish()
    }
}

/// A ring whose cursors and slots live in externally supplied memory.
///
/// # Safety contract
///
/// * `header` and `slots` must remain valid for the lifetime of use, mapped
///   shared (`MAP_SHARED` / shm) if endpoints live in different processes,
///   and zero-initialized before first attach.
/// * Exactly one producer thread/process and one consumer thread/process may
///   operate on the ring.
pub struct SharedSpsc<T> {
    header: *mut SharedHeader,
    slots: *mut MaybeUninit<T>,
    cap: usize,
}

// SAFETY: same argument as SpscRing — atomic-mediated slot exclusivity. The
// raw pointers are addresses of shared state never dereferenced outside the
// SPSC protocol; T: Send required for cross-thread transfer.
unsafe impl<T: Send> Send for SharedSpsc<T> {}
// SAFETY: &SharedSpsc exposes push/pop through correctly ordered atomics;
// slot exclusivity follows from the SPSC protocol (see type-level contract).
unsafe impl<T: Send> Sync for SharedSpsc<T> {}

impl<T> SharedSpsc<T> {
    /// Attaches to an initialized region.
    ///
    /// # Safety
    ///
    /// See type-level contract. `capacity` must match the layout used to size
    /// the region ([`crate::layout::RingLayout::for_slots`]).
    pub unsafe fn attach(
        header: *mut SharedHeader,
        slots: *mut MaybeUninit<T>,
        capacity: usize,
    ) -> Self {
        debug_assert!(
            capacity.is_power_of_two(),
            "capacity must be a power of two"
        );
        let cap = capacity.max(1).next_power_of_two();
        Self { header, slots, cap }
    }

    /// Maximum in-flight elements.
    #[inline]
    pub fn capacity(&self) -> usize {
        self.cap
    }

    #[inline]
    fn head(&self) -> &AtomicUsize {
        // SAFETY: header valid per attach() contract.
        unsafe { &(*self.header).head }
    }

    #[inline]
    fn tail(&self) -> &AtomicUsize {
        // SAFETY: header valid per attach() contract.
        unsafe { &(*self.header).tail }
    }

    /// Current element count (same caveat as [`SpscRing::len`](crate::SpscRing::len)).
    #[inline]
    pub fn len(&self) -> usize {
        self.tail()
            .load(Ordering::Acquire)
            .wrapping_sub(self.head().load(Ordering::Acquire))
    }

    /// True when no elements are pending.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Enqueues `value`; `Err(value)` when full. Wait-free.
    #[inline]
    pub fn push(&self, value: T) -> Result<(), T> {
        // SAFETY: SPSC protocol upheld by caller topology; pointers valid.
        unsafe {
            do_push(
                self.head(),
                self.tail(),
                self.slots,
                self.cap - 1,
                self.cap,
                value,
            )
        }
    }

    /// Dequeues the oldest element; `None` when empty. Wait-free.
    #[inline]
    pub fn pop(&self) -> Option<T> {
        // SAFETY: SPSC protocol upheld by caller topology; pointers valid.
        unsafe { do_pop(self.head(), self.tail(), self.slots, self.cap - 1) }
    }
}

impl<T> fmt::Debug for SharedSpsc<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SharedSpsc")
            .field("capacity", &self.cap)
            .field("len", &self.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;

    #[test]
    fn attached_region_cross_thread_roundtrip() {
        let header = Box::leak(Box::new(SharedHeader::new()));
        let slots: Box<[MaybeUninit<u64>]> = (0..8).map(|_| MaybeUninit::uninit()).collect();
        let slots = Box::leak(slots);

        // SAFETY: freshly zeroed region; capacity matches allocation.
        let ring = unsafe { SharedSpsc::attach(header, slots.as_mut_ptr(), 8) };
        assert!(ring.is_empty());
        for i in 0..8u64 {
            assert!(ring.push(i * i).is_ok());
        }
        assert!(ring.push(999).is_err()); // full at capacity
        assert_eq!(ring.pop(), Some(0));
        assert!(ring.push(64).is_ok());

        let shared = Arc::new(ring);
        let p = Arc::clone(&shared);
        let c = Arc::clone(&shared);
        let prod = thread::spawn(move || {
            for v in 100..200u64 {
                while p.push(v).is_err() {
                    std::hint::spin_loop();
                }
            }
        });
        let cons = thread::spawn(move || {
            // Sentinel-based termination: drain until the final produced
            // value (199) shows up, tolerating the pre-seeded backlog.
            let mut last = None;
            while last != Some(199) {
                match c.pop() {
                    Some(v) => last = Some(v),
                    None => std::hint::spin_loop(),
                }
            }
            last
        });
        prod.join().unwrap();
        assert_eq!(cons.join().unwrap(), Some(199));
        assert_eq!(header.head.load(Ordering::Acquire), 109);
    }
}
