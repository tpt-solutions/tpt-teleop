//! Cache-line padding to eliminate false sharing between hot atomics.

use core::fmt;

/// Size of one cache line on all supported targets (x86-64, aarch64).
pub const CACHE_LINE: usize = 64;

/// Wraps a value so it occupies its own 64-byte cache line.
///
/// Producer and consumer cursors of a ring live on separate lines so their
/// owning threads never ping-pong ownership of a shared line.
#[repr(align(64))]
#[derive(Clone, Copy, Default, Hash, PartialEq, Eq)]
pub struct CachePadded<T>(pub T);

impl<T> From<T> for CachePadded<T> {
    fn from(value: T) -> Self {
        Self(value)
    }
}

impl<T: fmt::Debug> fmt::Debug for CachePadded<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::mem::{align_of, size_of};

    #[test]
    fn occupies_full_cache_line() {
        assert_eq!(align_of::<CachePadded<usize>>(), CACHE_LINE);
        assert_eq!(size_of::<CachePadded<u64>>(), CACHE_LINE);
    }

    #[test]
    fn transparent_access() {
        let padded = CachePadded::from(42u32);
        assert_eq!(padded.0, 42);
    }
}
