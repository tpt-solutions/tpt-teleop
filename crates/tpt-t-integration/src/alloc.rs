//! Allocation-counting tooling for verifying the zero-heap hot path.
//!
//! `CountingAllocator` is a transparent `GlobalAlloc` wrapper around the
//! system allocator that tallies every `alloc` / `dealloc` / `realloc`. A test
//! binary opts in by declaring it as its `#[global_allocator]`:
//!
//! ```ignore
//! use tpt_t_integration::CountingAllocator;
//! #[global_allocator]
//! static A: CountingAllocator = CountingAllocator;
//! ```
//!
//! The counters are process-global atomics, so `reset_counts` is called once
//! after warm-up (construction allocations excluded) and `counts` is read
//! after the measured loop. A hot path that neither grows nor churns the heap
//! yields `allocs == deallocs == 0` inside the measured window.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicU64, Ordering};

static ALLOCS: AtomicU64 = AtomicU64::new(0);
static DEALLOCS: AtomicU64 = AtomicU64::new(0);
static REALLOCS: AtomicU64 = AtomicU64::new(0);
static BYTES: AtomicU64 = AtomicU64::new(0);

/// Reset all allocation counters to zero (call after warm-up).
pub fn reset_counts() {
    ALLOCS.store(0, Ordering::SeqCst);
    DEALLOCS.store(0, Ordering::SeqCst);
    REALLOCS.store(0, Ordering::SeqCst);
    BYTES.store(0, Ordering::SeqCst);
}

/// Snapshot of the allocation tallies.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AllocCounts {
    /// Total `alloc` calls since the last reset.
    pub allocs: u64,
    /// Total `dealloc` calls since the last reset.
    pub deallocs: u64,
    /// Total `realloc` calls since the last reset.
    pub reallocs: u64,
    /// Cumulative bytes requested by `alloc`/`alloc_zeroed`.
    pub bytes: u64,
}

impl AllocCounts {
    /// Net heap growth (`allocs + reallocs − deallocs`) — the real-time
    /// invariant is `net == 0`: the hot path must not grow the heap.
    pub fn net_allocations(self) -> i64 {
        self.allocs as i64 + self.reallocs as i64 - self.deallocs as i64
    }
}

/// Read the current tallies.
pub fn counts() -> AllocCounts {
    AllocCounts {
        allocs: ALLOCS.load(Ordering::SeqCst),
        deallocs: DEALLOCS.load(Ordering::SeqCst),
        reallocs: REALLOCS.load(Ordering::SeqCst),
        bytes: BYTES.load(Ordering::SeqCst),
    }
}

/// Transparent counting allocator. Set as a test binary's `#[global_allocator]`.
pub struct CountingAllocator;

// SAFETY: every method forwards exactly to the system allocator with the same
// layout/pointer contract; the counters are only incremented on the side.
unsafe impl GlobalAlloc for CountingAllocator {
    #[inline]
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::SeqCst);
        BYTES.fetch_add(layout.size() as u64, Ordering::SeqCst);
        // SAFETY: `layout` is the caller's validated layout, forwarded verbatim.
        unsafe { System.alloc(layout) }
    }

    #[inline]
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        DEALLOCS.fetch_add(1, Ordering::SeqCst);
        // SAFETY: `ptr`/`layout` are the pair the allocator handed out.
        unsafe { System.dealloc(ptr, layout) }
    }

    #[inline]
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::SeqCst);
        BYTES.fetch_add(layout.size() as u64, Ordering::SeqCst);
        // SAFETY: `layout` is the caller's validated layout, forwarded verbatim.
        unsafe { System.alloc_zeroed(layout) }
    }

    #[inline]
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        REALLOCS.fetch_add(1, Ordering::SeqCst);
        // SAFETY: `ptr`/`layout`/`new_size` are the caller's validated args.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}
