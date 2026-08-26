//! The wait-free SPSC ring buffer.
//!
//! # Algorithm
//!
//! Two monotonic cursors, each owned exclusively by one endpoint:
//!
//! * `tail` — written only by the producer (Release after publishing a slot).
//! * `head` — written only by the consumer (Release after consuming a slot).
//!
//! Each side reads the other's cursor with Acquire ordering to establish
//! happens-before edges for slot contents. Neither operation loops, locks,
//! or allocates: a failed `push` returns `Err(value)` immediately (ring
//! full), a failed `pop` returns `None` (ring empty). Both operations are
//! therefore *wait-free* with O(1) worst-case instruction counts — the
//! property the <10 µs safety intercept budget depends on.
//!
//! Wrap-around uses wrapping subtraction of monotonic counters; correctness
//! holds across the full `usize` range since capacity is a power of two.

use core::cell::UnsafeCell;
use core::fmt;
use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicUsize, Ordering};

use crate::cache_line::CachePadded;

/// Producer-side half of the hot algorithm, shared by heap and shm backends.
///
/// # Safety
///
/// Caller must guarantee: single producer thread; `slots` points at `cap`
/// contiguous `MaybeUninit<T>` cells; `head`/`tail` are the paired cursors
/// of the same ring instance.
#[inline]
pub(crate) unsafe fn do_push<T>(
    head: &AtomicUsize,
    tail: &AtomicUsize,
    slots: *mut MaybeUninit<T>,
    mask: usize,
    cap: usize,
    value: T,
) -> Result<(), T> {
    let t = tail.load(Ordering::Relaxed);
    // Acquire pairs with consumer's Release store of `head`: we observe
    // exactly which slots were freed before claiming ours.
    if t.wrapping_sub(head.load(Ordering::Acquire)) >= cap {
        return Err(value); // Full: fail fast, wait-free.
    }
    // SAFETY: t is this producer's exclusive claim; t - head < cap proves the
    // cell at t & mask was fully consumed before us.
    unsafe { (*slots.add(t & mask)).write(value) };
    // Release publishes slot contents before the cursor becomes visible.
    tail.store(t.wrapping_add(1), Ordering::Release);
    Ok(())
}

/// Consumer-side half of the hot algorithm. Symmetric to [`do_push`].
///
/// # Safety
///
/// Same contract as [`do_push`], mirrored for the single consumer thread.
#[inline]
pub(crate) unsafe fn do_pop<T>(
    head: &AtomicUsize,
    tail: &AtomicUsize,
    slots: *mut MaybeUninit<T>,
    mask: usize,
) -> Option<T> {
    let h = head.load(Ordering::Relaxed);
    // Acquire pairs with producer's Release store of `tail`.
    if h == tail.load(Ordering::Acquire) {
        return None; // Empty: fail fast, wait-free.
    }
    // SAFETY: h != tail proves the cell at h & mask was fully published.
    let value = unsafe { (*slots.add(h & mask)).assume_init_read() };
    head.store(h.wrapping_add(1), Ordering::Release);
    Some(value)
}

/// Heap-backed, wait-free, bounded single-producer/single-consumer ring.
///
/// Share as `Arc<SpscRing<T>>`: exactly one thread pushes, one pops. Any
/// other topology is a caller bug and will corrupt state.
pub struct SpscRing<T> {
    slots: Box<[UnsafeCell<MaybeUninit<T>>]>,
    head: CachePadded<AtomicUsize>, // consumer cursor: next index to pop
    tail: CachePadded<AtomicUsize>, // producer cursor: next index to push
    cap: usize,                     // power of two
}

// SAFETY: T crosses threads by design (producer hands ownership to the
// consumer); sound iff T: Send. Slot exclusivity is enforced by the SPSC
// protocol mediated by atomic cursors (see module docs).
unsafe impl<T: Send> Send for SpscRing<T> {}
// SAFETY: &SpscRing is the intended sharing mode (Arc across two threads);
// interior mutation flows only through correctly-ordered atomics.
unsafe impl<T: Send> Sync for SpscRing<T> {}

impl<T> SpscRing<T> {
    /// Creates a ring holding at least `capacity` elements.
    /// Capacity is rounded up to a power of two (minimum 1).
    pub fn with_capacity(capacity: usize) -> Self {
        let cap = capacity.max(1).next_power_of_two();
        let slots: Box<[UnsafeCell<MaybeUninit<T>>]> = (0..cap)
            .map(|_| UnsafeCell::new(MaybeUninit::uninit()))
            .collect();
        Self {
            slots,
            head: CachePadded(AtomicUsize::new(0)),
            tail: CachePadded(AtomicUsize::new(0)),
            cap,
        }
    }

    /// Maximum number of in-flight elements (power of two).
    #[inline]
    pub fn capacity(&self) -> usize {
        self.cap
    }

    /// Current element count. Exact when only one endpoint calls it;
    /// racy-but-bounded while both threads operate concurrently.
    #[inline]
    pub fn len(&self) -> usize {
        self.tail
            .0
            .load(Ordering::Acquire)
            .wrapping_sub(self.head.0.load(Ordering::Acquire))
    }

    /// True when no elements are pending.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// True when a subsequent [`push`](Self::push) would fail.
    #[inline]
    pub fn is_full(&self) -> bool {
        self.len() >= self.cap
    }

    /// Enqueues `value`; returns `Err(value)` if the ring is full.
    /// Wait-free: fixed instruction sequence, never spins.
    #[inline]
    pub fn push(&self, value: T) -> Result<(), T> {
        // SAFETY: single-producer protocol by type contract; slots, cursors,
        // and cap are internally consistent by construction.
        unsafe {
            do_push(
                &self.head.0,
                &self.tail.0,
                self.slots.as_ptr() as *mut MaybeUninit<T>,
                self.cap - 1,
                self.cap,
                value,
            )
        }
    }

    /// Dequeues the oldest element, or `None` if empty. Wait-free.
    #[inline]
    pub fn pop(&self) -> Option<T> {
        // SAFETY: single-consumer protocol; see push().
        unsafe {
            do_pop(
                &self.head.0,
                &self.tail.0,
                self.slots.as_ptr() as *mut MaybeUninit<T>,
                self.cap - 1,
            )
        }
    }
}

impl<T: Copy> SpscRing<T> {
    /// Pushes as many elements of `slice` as fit; returns the count moved.
    /// Batch convenience for telemetry bursts; per-element semantics equal
    /// repeated [`push`](Self::push) calls.
    pub fn push_slice(&self, slice: &[T]) -> usize {
        let mut n = 0;
        while n < slice.len() && self.push(slice[n]).is_ok() {
            n += 1;
        }
        n
    }
}

impl<T> Drop for SpscRing<T> {
    fn drop(&mut self) {
        // Drain so unconsumed values run destructors exactly once.
        while self.pop().is_some() {}
    }
}

impl<T> fmt::Debug for SpscRing<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SpscRing")
            .field("capacity", &self.cap)
            .field("len", &self.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize as AU, Ordering as AO};
    use std::thread;

    #[test]
    fn fifo_ordering_single_thread() {
        let ring: SpscRing<u64> = SpscRing::with_capacity(128);
        assert!(ring.is_empty());

        // Pure fill-then-drain preserves FIFO exactly.
        for i in 0..100u64 {
            ring.push(i).expect("capacity holds the whole run");
        }
        let drained: Vec<u64> = core::iter::from_fn(|| ring.pop()).collect();
        assert_eq!(drained, (0..100).collect::<Vec<_>>());
        assert!(ring.is_empty());

        // Interleaved pattern: one push per iteration, one pop every second
        // iteration — drains in exact FIFO order with two in flight.
        let mut expect = 0u64;
        for i in 0..200u64 {
            ring.push(i).unwrap();
            if i % 2 == 1 {
                assert_eq!(ring.pop(), Some(expect));
                expect += 1;
            }
        }
        // Drain the 100 still-in-flight values.
        while let Some(v) = ring.pop() {
            assert_eq!(v, expect);
            expect += 1;
        }
        assert_eq!(expect, 200);
        assert_eq!(ring.pop(), None);
    }

    #[test]
    fn full_and_empty_semantics() {
        let ring: SpscRing<u32> = SpscRing::with_capacity(4);
        assert_eq!(ring.capacity(), 4);
        for i in 0..4 {
            assert!(ring.push(i).is_ok());
        }
        assert!(ring.is_full());
        assert_eq!(ring.push(99), Err(99)); // rejected value handed straight back
        assert_eq!(ring.len(), 4);
        assert_eq!(ring.pop(), Some(0));
        assert!(!ring.is_full());
        assert!(ring.push(99).is_ok());
    }

    #[test]
    fn capacity_rounds_to_power_of_two() {
        let ring: SpscRing<u8> = SpscRing::with_capacity(5);
        assert_eq!(ring.capacity(), 8);
        let tiny: SpscRing<u8> = SpscRing::with_capacity(0);
        assert_eq!(tiny.capacity(), 1);
    }

    #[test]
    fn cross_thread_producer_consumer() {
        const N: u64 = 200_000;
        let ring = Arc::new(SpscRing::<u64>::with_capacity(1024));
        let prod_ring = Arc::clone(&ring);

        let producer = thread::spawn(move || {
            for i in 0..N {
                while prod_ring.push(i).is_err() {
                    std::hint::spin_loop(); // backpressure only, never blocks
                }
            }
        });
        let consumer = thread::spawn(move || {
            let mut sum = 0u64;
            let mut got = 0u64;
            while got < N {
                if let Some(v) = ring.pop() {
                    sum = sum.wrapping_add(v);
                    got += 1;
                } else {
                    std::hint::spin_loop();
                }
            }
            (got, sum)
        });

        producer.join().unwrap();
        let (got, sum) = consumer.join().unwrap();
        assert_eq!(got, N);
        assert_eq!(sum, N * (N - 1) / 2);
    }

    #[test]
    fn sequential_pair_handoff_keeps_order() {
        // Multiple producer/consumer pairs run one after another against the
        // same ring ("multi producer-consumer pairs" scenario): order is
        // preserved within each sequential pair.
        let ring = Arc::new(SpscRing::<usize>::with_capacity(16));
        for pair in 0..4 {
            let base = pair * 1000;
            let w = Arc::clone(&ring);
            let p = thread::spawn(move || {
                for i in 0..500 {
                    while w.push(base + i).is_err() {
                        std::hint::spin_loop();
                    }
                }
            });
            let r = Arc::clone(&ring);
            let c = thread::spawn(move || {
                for i in 0..500 {
                    loop {
                        if let Some(v) = r.pop() {
                            assert_eq!(v, base + i);
                            break;
                        }
                        std::hint::spin_loop();
                    }
                }
            });
            p.join().unwrap();
            c.join().unwrap();
        }
        assert!(ring.is_empty());
    }

    #[test]
    fn unconsumed_values_drop_exactly_once() {
        static DROPS: AU = AU::new(0);
        #[derive(Debug)]
        struct Counted(#[allow(dead_code)] u32);
        impl Drop for Counted {
            fn drop(&mut self) {
                DROPS.fetch_add(1, AO::SeqCst);
            }
        }

        {
            let ring: SpscRing<Counted> = SpscRing::with_capacity(8);
            for i in 0..5 {
                ring.push(Counted(i)).unwrap();
            }
            assert_eq!(ring.pop().unwrap().0, 0);
            assert_eq!(DROPS.load(AO::SeqCst), 1);
            // 4 remain inside the ring; dropping it must drop them too.
        }
        assert_eq!(DROPS.load(AO::SeqCst), 5);
    }

    #[test]
    fn push_slice_moves_prefix() {
        let ring: SpscRing<u16> = SpscRing::with_capacity(4);
        let moved = ring.push_slice(&[1, 2, 3, 4, 5, 6]);
        assert_eq!(moved, 4);
        assert_eq!(ring.pop(), Some(1));
        assert_eq!(ring.pop(), Some(2));
    }
}
