//! Zero-copy struct casting: byte slices ↔ plain-old-data structs.
//!
//! This is the "Normalize" step of spec §6: a raw HID report (or any wire
//! blob whose layout both ends agree on) is reinterpreted as a struct with
//! no copies and no allocations.
//!
//! # Safety contract for [`PlainBytes`]
//!
//! Implementors must be `#[repr(C)]` (or `#[repr(transparent)]`) types that:
//! 1. contain no padding bytes,
//! 2. have no invalid bit patterns (every bit pattern is a valid value),
//! 3. are `Copy`.
//!
//! Primitives (`u*`, `i*`, `f*`, `usize`, `isize`) satisfy this. `bool`,
//! `char`, enums, references, and pointers do **not** and are intentionally
//! excluded. User structs opt in by writing an explicit
//! `unsafe impl PlainBytes for MyStruct {}` after verifying the rules above.

use core::mem::{align_of, size_of};

/// Marker trait for types safely castable to/from raw bytes.
///
/// # Safety
///
/// Implementors must be `#[repr(C)]` (or `#[repr(transparent)]`) with no
/// padding bytes, no invalid bit patterns, and `Copy`. Violating any of the
/// three makes [`bytes_of`] / [`ref_from_bytes`] unsound.
pub unsafe trait PlainBytes: Copy {}

macro_rules! impl_plain_bytes {
    ($($t:ty),* $(,)?) => {$(
        // SAFETY: fixed-width integers/floats have no padding and no
        // invalid bit patterns (NaN payloads included for floats).
        unsafe impl PlainBytes for $t {}
    )*};
}

impl_plain_bytes!(
    u8, u16, u32, u64, u128, usize, i8, i16, i32, i64, i128, isize, f32, f64
);

// SAFETY: arrays of PlainBytes elements keep the no-padding/no-invalid-bit
// guarantees element-wise; [T; N] layout is dense (no interior padding).
unsafe impl<T: PlainBytes, const N: usize> PlainBytes for [T; N] {}

/// Errors returned when a byte slice cannot be cast to `T`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CastError {
    /// Slice is shorter than `size_of::<T>()`.
    TooShort { needed: usize, got: usize },
    /// Slice start pointer is not aligned to `align_of::<T>()`.
    Misaligned { needed: usize },
}

impl core::fmt::Display for CastError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            CastError::TooShort { needed, got } => {
                write!(f, "byte slice too short: need {needed} bytes, got {got}")
            }
            CastError::Misaligned { needed } => {
                write!(f, "byte slice misaligned: alignment {needed} required")
            }
        }
    }
}

impl std::error::Error for CastError {}

/// Borrows `v` as its raw bytes. No copy.
#[inline]
pub fn bytes_of<T: PlainBytes>(v: &T) -> &[u8] {
    // SAFETY: PlainBytes guarantees no padding, so all size_of::<T>() bytes
    // of the object are initialized u8 data.
    unsafe { core::slice::from_raw_parts(v as *const T as *const u8, size_of::<T>()) }
}

/// Mutably borrows `v` as raw bytes. No copy.
///
/// Writing arbitrary bit patterns is safe because PlainBytes types are valid
/// for every bit pattern.
#[inline]
pub fn bytes_of_mut<T: PlainBytes>(v: &mut T) -> &mut [u8] {
    // SAFETY: see above; every bit pattern yields a valid PlainBytes value.
    unsafe { core::slice::from_raw_parts_mut(v as *mut T as *mut u8, size_of::<T>()) }
}

/// Reinterprets the prefix of `bytes` as `&T` without copying.
#[inline]
pub fn ref_from_bytes<T: PlainBytes>(bytes: &[u8]) -> Result<&T, CastError> {
    check_len::<T>(bytes)?;
    check_align::<T>(bytes)?;
    // SAFETY: length >= size_of::<T>(), alignment satisfied, PlainBytes
    // makes any bit pattern valid; result lifetime tied to input slice.
    Ok(unsafe { &*(bytes.as_ptr() as *const T) })
}

/// Mutable variant of [`ref_from_bytes`].
#[inline]
pub fn ref_from_bytes_mut<T: PlainBytes>(bytes: &mut [u8]) -> Result<&mut T, CastError> {
    check_len::<T>(bytes)?;
    check_align::<T>(bytes)?;
    // SAFETY: as ref_from_bytes, plus unique &mut guarantees exclusivity.
    Ok(unsafe { &mut *(bytes.as_mut_ptr() as *mut T) })
}

/// Casts the prefix to `T` and returns it alongside the remaining suffix —
/// stacked-header parsing in one pass.
#[inline]
pub fn split_prefix<T: PlainBytes>(bytes: &[u8]) -> Result<(&T, &[u8]), CastError> {
    check_len::<T>(bytes)?;
    check_align::<T>(bytes)?;
    // SAFETY: same preconditions as ref_from_bytes.
    let head = unsafe { &*(bytes.as_ptr() as *const T) };
    Ok((head, &bytes[size_of::<T>()..]))
}

#[inline]
fn check_len<T: PlainBytes>(bytes: &[u8]) -> Result<(), CastError> {
    let needed = size_of::<T>();
    if bytes.len() < needed {
        return Err(CastError::TooShort {
            needed,
            got: bytes.len(),
        });
    }
    Ok(())
}

#[inline]
fn check_align<T: PlainBytes>(bytes: &[u8]) -> Result<(), CastError> {
    let needed = align_of::<T>();
    if !bytes.as_ptr().cast::<T>().is_aligned() {
        return Err(CastError::Misaligned { needed });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primitive_roundtrip() {
        let v: u32 = 0xDEAD_BEEF;
        let bytes = bytes_of(&v);
        assert_eq!(bytes.len(), 4);
        let back: &u32 = ref_from_bytes(bytes).unwrap();
        assert_eq!(*back, v);
    }

    #[test]
    fn float_and_array_roundtrip() {
        let f = 3.125f64;
        assert_eq!(ref_from_bytes::<f64>(bytes_of(&f)).unwrap(), &f);

        let arr = [1u16, 2, 3, 4];
        let mut buf = bytes_of(&arr).to_vec();
        buf.extend_from_slice(&[0xAA, 0xBB]);
        let (back, suffix): (&[u16; 4], &[u8]) = split_prefix(&buf).unwrap();
        assert_eq!(back, &arr);
        assert_eq!(suffix, &[0xAA, 0xBB]);
    }

    #[test]
    fn rejects_short_buffer() {
        let buf = [0u8; 3];
        assert_eq!(
            ref_from_bytes::<u32>(&buf),
            Err(CastError::TooShort { needed: 4, got: 3 })
        );
    }

    #[test]
    fn rejects_misaligned() {
        let buf = [0u8; 16];
        let (_, unaligned) = buf.split_at(1); // offset by one byte
        assert!(matches!(
            ref_from_bytes::<u32>(unaligned),
            Err(CastError::Misaligned { .. })
        ));
    }

    #[test]
    fn mutable_cast_writes_through() {
        let mut v = 7u64;
        let raw = bytes_of_mut(&mut v);
        raw[7] = 9; // top byte on LE targets
        assert_eq!(*ref_from_bytes::<u64>(bytes_of(&v)).unwrap(), v);
    }

    #[repr(C)]
    #[derive(Clone, Copy, Debug, PartialEq)]
    struct Packet {
        magic: u16,
        len: u16,
        payload: [u8; 6],
    }
    // SAFETY: repr(C); u16+u16+[u8;6] pack densely to 10 bytes with no
    // padding; every bit pattern of these fields is valid.
    unsafe impl PlainBytes for Packet {}

    #[test]
    fn user_struct_roundtrip() {
        let p = Packet {
            magic: 0xCAFE,
            len: 6,
            payload: *b"TELEOP",
        };
        let bytes = bytes_of(&p);
        assert_eq!(bytes.len(), 10);
        let q: &Packet = ref_from_bytes(bytes).unwrap();
        assert_eq!(*q, p);
    }
}
