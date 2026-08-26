//! Shared-memory allocation strategy for ring buffers.
//!
//! A ring is laid out as one contiguous region so it can live in a SysV/POSIX
//! shm segment, a `mmap`ed file, or a plain heap allocation interchangeably:
//!
//! ```text
//! offset 0                    slots_off                 total_len
//! ┌──────────────────────────┬─────────────────────────┐
//! │ SharedHeader             │ slot[0] … slot[cap-1]   │
//! │ head | pad | tail | pad  │ slot_size-aligned cells │
//! └──────────────────────────┴─────────────────────────┘
//! ◄──── 64-byte aligned ────►◄─── slot_size aligned ───►
//! ```
//!
//! Both endpoints map the same region read/write; cursors are atomics so no
//! additional synchronization primitives are required. [`RingLayout`]
//! computes all offsets deterministically from `(T, capacity)` and
//! [`RingLayout::fit`] answers the inverse question ("largest power-of-two
//! capacity inside a given region"), which is what shm-attach code paths use
//! when the region size is dictated externally.

use crate::cache_line::CACHE_LINE;
use crate::shared::SharedHeader;
use core::mem::size_of;

/// Deterministic byte layout of one ring instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RingLayout {
    /// Byte length of [`SharedHeader`] (two cache-padded cursors).
    pub header_len: usize,
    /// Offset of `slot[0]`; 64-byte aligned.
    pub slots_off: usize,
    /// Per-slot stride: `size_of::<T>()` rounded up to 8 bytes.
    pub slot_size: usize,
    /// Slot count; always a power of two.
    pub capacity: usize,
    /// Total region size: `slots_off + slot_size * capacity`.
    pub total_len: usize,
}

impl RingLayout {
    /// Computes the canonical layout for `capacity` slots of `T`.
    /// `capacity` is rounded up to a power of two (min 1).
    pub fn for_slots<T>(capacity: usize) -> Self {
        let cap = capacity.max(1).next_power_of_two();
        let header_len = size_of::<SharedHeader>();
        let slot_size = (size_of::<T>().max(1)).div_ceil(8) * 8;
        let slots_off = header_len.div_ceil(CACHE_LINE) * CACHE_LINE;
        let total_len = slots_off + slot_size * cap;
        Self {
            header_len,
            slots_off,
            slot_size,
            capacity: cap,
            total_len,
        }
    }

    /// Largest power-of-two capacity of `slot_size`-byte slots that fits in a
    /// region of `total_len` bytes with `header_len` header bytes.
    /// Returns `None` if not even one slot fits. Rounds **down** — the
    /// resulting ring never exceeds the region it was sized for.
    pub fn fit(total_len: usize, header_len: usize, slot_size: usize) -> Option<usize> {
        let slots_off = header_len.div_ceil(CACHE_LINE) * CACHE_LINE;
        let avail = total_len.checked_sub(slots_off)?;
        let slot_size = slot_size.max(1);
        let n = avail / slot_size;
        Some(floor_pow2(n))
    }

    /// Validates that a region of `total_len` bytes can host this layout.
    pub fn validate(&self, total_len: usize) -> bool {
        self.slots_off % CACHE_LINE == 0
            && self.capacity.is_power_of_two()
            && total_len >= self.total_len
    }
}

/// Largest power of two ≤ `n`; zero for `n == 0`.
fn floor_pow2(n: usize) -> usize {
    if n.is_power_of_two() {
        n
    } else {
        1 << (usize::BITS - 1 - n.leading_zeros())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_is_deterministic_and_aligned() {
        let l = RingLayout::for_slots::<u32>(1000);
        assert_eq!(l.capacity, 1024);
        assert!(l.slots_off % CACHE_LINE == 0);
        assert_eq!(l.slot_size, 8); // 4-byte payload padded to 8-byte stride
        assert_eq!(l.total_len, l.slots_off + 8 * 1024);
        assert!(l.validate(l.total_len));
        assert!(!l.validate(l.total_len - 1));
    }

    #[test]
    fn fit_is_inverse_of_for_slots() {
        let l = RingLayout::for_slots::<u64>(256);
        let fitted = RingLayout::fit(l.total_len, l.header_len, 8).unwrap();
        assert_eq!(fitted, 256);
        // One byte short: usable slots drop to 255 -> largest power of two
        // that still fits is 128.
        let tight = RingLayout::fit(l.total_len - 1, l.header_len, 8).unwrap();
        assert_eq!(tight, 128);
    }

    #[test]
    fn fit_never_exceeds_region() {
        // For every probed region size the fitted ring's own footprint must
        // fit back inside the original budget.
        for total in [4096usize, 8192, 65_536] {
            let cap = RingLayout::fit(total, 128, 16).unwrap();
            let l = RingLayout::for_slots::<[u8; 16]>(cap);
            assert!(
                l.total_len <= total,
                "cap {cap} needs {} > {total}",
                l.total_len
            );
        }
    }

    #[test]
    fn fit_tiny_region() {
        assert_eq!(RingLayout::fit(16, 8, 8), None); // no room past header
        // 4096-128=3968 usable; /16 = 248 slots -> floors to 128.
        assert_eq!(RingLayout::fit(4096, 128, 16), Some(128));
    }
}
