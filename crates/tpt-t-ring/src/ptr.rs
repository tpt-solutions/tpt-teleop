//! Zero-copy pointer-passing rings ("Route" step of spec §6).
//!
//! Instead of moving large payloads through the queue, producers allocate
//! from a pool/slab (see tpt-t-core's buffer pool, tpt-t-media's
//! frame slabs) and hand over the raw pointer. The consumer owns the buffer
//! until it returns it to the pool. Payload bytes are never copied.
//!
//! [`Ptr`] is the ownership token crossing the ring; [`PointerRing`] is a
//! plain [`SpscRing`](crate::SpscRing) over those tokens.

use core::fmt;

use crate::spsc::SpscRing;

/// An owned raw-pointer token transferred across a [`PointerRing`].
///
/// # Safety contract
///
/// Whoever pops a `Ptr` takes exclusive ownership of `pointee`'s
/// `len` bytes exactly as if the producer had moved a `&mut [u8]`.
/// Producers must hand out each pointer at most once until it is recycled
/// by the owning pool.
pub struct Ptr(pub *mut u8);

// SAFETY: Ptr is an explicit ownership token; transferring it between
// threads is the entire point. Pointee validity is the pool's invariant.
unsafe impl Send for Ptr {}
// SAFETY: &Ptr is just a shared view of the token value itself; handing out
// copies of the token is the producer's contract (each handed at most once).
unsafe impl Sync for Ptr {}

impl fmt::Debug for Ptr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Ptr({:p})", self.0)
    }
}

/// Convenience alias: wait-free SPSC ring of pointer tokens.
pub type PointerRing = SpscRing<Ptr>;

/// Ergonomic push/pop for raw pointers over a [`PointerRing`].
pub trait PointerRingExt {
    /// Pushes a pointer token; `Err(ptr)` back when full.
    fn push_ptr(&self, ptr: *mut u8) -> Result<(), *mut u8>;

    /// Pops a pointer token, taking ownership of the pointee.
    fn pop_ptr(&self) -> Option<*mut u8>;
}

impl PointerRingExt for PointerRing {
    #[inline]
    fn push_ptr(&self, ptr: *mut u8) -> Result<(), *mut u8> {
        self.push(Ptr(ptr)).map_err(|e| e.0)
    }

    #[inline]
    fn pop_ptr(&self) -> Option<*mut u8> {
        self.pop().map(|p| p.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn pointer_roundtrip_cross_thread() {
        let ring: PointerRing = SpscRing::with_capacity(4);
        let backing = [0u8; 16];
        let base = backing.as_ptr() as *mut u8;

        ring.push_ptr(base).unwrap();
        // SAFETY: stays within the 16-byte backing array.
        ring.push_ptr(unsafe { base.add(8) }).unwrap();
        // SAFETY: stays within the 16-byte backing array.
        ring.push_ptr(unsafe { base.add(4) }).unwrap();

        let handle = thread::spawn(move || {
            let mut seen = Vec::new();
            while seen.len() < 3 {
                if let Some(p) = ring.pop_ptr() {
                    seen.push(p as usize);
                } else {
                    std::hint::spin_loop();
                }
            }
            seen
        });

        let seen = handle.join().unwrap();
        assert_eq!(seen[0], base as usize);
        // SAFETY: offsets stay inside the backing array (pointer arithmetic
        // on the captured base, compared as addresses only).
        assert_eq!(seen[1], unsafe { base.add(8) } as usize);
        // SAFETY: same array-bounds argument as above.
        assert_eq!(seen[2], unsafe { base.add(4) } as usize);
    }

    #[test]
    fn full_ring_returns_pointer_back() {
        let ring: PointerRing = SpscRing::with_capacity(1);
        let x = [0u8; 4];
        let p = x.as_ptr() as *mut u8;
        assert!(ring.push_ptr(p).is_ok());
        assert_eq!(ring.push_ptr(p), Err(p)); // handed straight back
        assert_eq!(ring.pop_ptr(), Some(p));
    }
}
