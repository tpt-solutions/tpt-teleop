//! Pre-allocated buffer pool — zero steady-state heap allocation.
//!
//! Every large buffer (video frames, UDP packet buffers, FDR blocks) comes
//! from a pool created once at startup. Acquisition hands out exclusive
//! access via a RAII [`Pooled`] guard whose `Drop` returns the slot to the
//! lock-free free-list (an [`MpmcRing`] of indices). Producers pair the pool
//! with pointer rings (tpt-t-ring::ptr): allocate → fill → push_ptr →
//! consumer pops → drops guard → slot recycled. No malloc after startup,
//! exactly the spec §3.4 discipline.

use core::fmt;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crate::mpmc::MpmcRing;

/// Fixed-capacity object pool.
///
/// Slots are uninitialized until first acquired-and-initialized via
/// [`BufferPool::get_with`]; subsequent acquisitions reuse stored values.
pub struct BufferPool<T> {
    slots: Box<[Slot<T>]>,
    free: MpmcRing<u32>,
    initialized: Box<[AtomicBool]>,
    outstanding: AtomicUsize,
}

struct Slot<T> {
    value: core::cell::UnsafeCell<core::mem::MaybeUninit<T>>,
}

// SAFETY: slot access is arbitrated by the free-list: a slot index is handed
// to exactly one owner at a time; Pooled guards provide &/&mut exclusively.
unsafe impl<T: Send> Send for BufferPool<T> {}
// SAFETY: &BufferPool hands out slots via the lock-free free-list; the same
// exclusivity argument covers concurrent acquirers.
unsafe impl<T: Send> Sync for BufferPool<T> {}

impl<T> BufferPool<T> {
    /// Creates a pool of `size` slots (capacity rounds up inside the
    /// underlying queue but `size` slots are allocated verbatim, min 1).
    pub fn new(size: usize) -> Self {
        let size = size.max(1);
        let slots: Box<[Slot<T>]> = (0..size)
            .map(|_| Slot {
                value: core::cell::UnsafeCell::new(core::mem::MaybeUninit::uninit()),
            })
            .collect();
        let free = MpmcRing::with_capacity(size);
        for i in 0..size as u32 {
            let _ = free.push(i);
        }
        Self {
            slots,
            free,
            initialized: (0..size).map(|_| AtomicBool::new(false)).collect(),
            outstanding: AtomicUsize::new(0),
        }
    }

    /// Total slot count.
    pub fn capacity(&self) -> usize {
        self.slots.len()
    }

    /// Slots currently checked out.
    pub fn in_use(&self) -> usize {
        self.outstanding.load(Ordering::Acquire)
    }

    /// Takes a slot out of the pool, initializing it with `init` on first
    /// use (later acquisitions see the previous contents via
    /// [`Pooled::write`] or [`DerefMut`]). `None` when exhausted.
    pub fn get_with<F: FnOnce() -> T>(&self, init: F) -> Option<Pooled<'_, T>> {
        let idx = self.free.pop()?;
        self.outstanding.fetch_add(1, Ordering::AcqRel);
        if !self.initialized[idx as usize].load(Ordering::Acquire) {
            // SAFETY: exclusive ownership via free-list claim; we store the
            // full MaybeUninit (initialized here) which later Deref paths
            // read back with assume_init_ref.
            unsafe {
                self.slot(idx)
                    .value
                    .get()
                    .write(core::mem::MaybeUninit::new(init()))
            };
            self.initialized[idx as usize].store(true, Ordering::Release);
        }
        Some(Pooled { pool: self, idx })
    }

    /// Convenience: `get_with(Default::default)` for `T: Default`.
    pub fn get_default(&self) -> Option<Pooled<'_, T>>
    where
        T: Default,
    {
        self.get_with(T::default)
    }

    #[inline]
    fn slot(&self, idx: u32) -> &Slot<T> {
        // SAFETY: indices issued by the free-list are always < slots.len().
        unsafe { self.slots.get_unchecked(idx as usize) }
    }

    #[inline]
    fn release(&self, idx: u32) {
        // SAFETY: caller (Pooled::drop) proved exclusive ownership.
        let _ = self.free.push(idx);
        self.outstanding.fetch_sub(1, Ordering::AcqRel);
    }
}

impl<T> fmt::Debug for BufferPool<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BufferPool")
            .field("capacity", &self.capacity())
            .field("in_use", &self.in_use())
            .finish()
    }
}

/// RAII handle to one pooled slot. Dereferences to `T`; dropping returns the
/// slot to the pool (wait-free enqueue onto the free-list).
pub struct Pooled<'a, T> {
    pool: &'a BufferPool<T>,
    idx: u32,
}

impl<T> Deref for Pooled<'_, T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &T {
        // SAFETY: initialized at first checkout; exclusive while guarded.
        unsafe { (*self.pool.slot(self.idx).value.get()).assume_init_ref() }
    }
}

impl<T> DerefMut for Pooled<'_, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: exclusive ownership guaranteed by the guard.
        unsafe { (*self.pool.slot(self.idx).value.get()).assume_init_mut() }
    }
}

impl<T> Drop for Pooled<'_, T> {
    fn drop(&mut self) {
        self.pool.release(self.idx);
    }
}

impl<T> fmt::Debug for Pooled<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Pooled").field("slot", &self.idx).finish()
    }
}
