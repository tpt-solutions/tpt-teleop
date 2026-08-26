//! Minimal NumPy `.npy` (v1.0) reader/writer.
//!
//! NumPy's `.npy` is the interchange format both PyTorch (`numpy.load` →
//! `torch.from_numpy`) and JAX (`numpy.load` → `jnp.asarray`) consume
//! natively, so it is the single export target for the AI training pipeline
//! (spec §5.8). We hand-roll the format (a 6-byte magic, a 2-byte version, a
//! 2-byte little-endian header length, a padded ASCII dict, then raw
//! little-endian tensor bytes) to avoid pulling any dependency that could
//! break the MIT-chain policy.

use std::io::{self, Write};

/// NumPy `.npy` magic prefix (`\x93NUMPY`).
pub const NPY_MAGIC: [u8; 6] = [0x93, b'N', b'U', b'M', b'P', b'Y'];

/// Errors from `.npy` parsing.
#[derive(Debug)]
pub enum NpyError {
    /// Stream shorter than the 10-byte fixed prefix.
    TooShort,
    /// Magic bytes did not match.
    BadMagic,
    /// Declared header length would run past the buffer.
    BadHeaderLen,
    /// Header was not valid UTF-8 / dict.
    BadHeader,
}

impl std::fmt::Display for NpyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NpyError::TooShort => write!(f, "npy: stream too short for prefix"),
            NpyError::BadMagic => write!(f, "npy: bad magic (not a .npy file)"),
            NpyError::BadHeaderLen => write!(f, "npy: header length out of range"),
            NpyError::BadHeader => write!(f, "npy: malformed header dict"),
        }
    }
}

impl std::error::Error for NpyError {}

/// Writes a tensor described by `descr` (e.g. `"<f4"`, `"<f8"`, `"<i8"`) and
/// `shape` with raw `data` bytes (must be `product(shape) * element_size`).
pub fn write_npy<W: Write>(w: &mut W, descr: &str, shape: &[usize], data: &[u8]) -> io::Result<()> {
    let shape_str = shape
        .iter()
        .map(|s| s.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    let dict = format!("{{'descr': '{descr}', 'fortran_order': False, 'shape': ({shape_str}), }}");

    // The 2-byte header-length field must be a multiple of 64 so that the
    // whole prefix (6 magic + 2 version + 2 length + header) lands on a 64-byte
    // boundary. NumPy requires header_length % 64 == 54 (since 10 + 54 = 64).
    let need = dict.len() + 1; // +1 for trailing newline
    let mut hl = 54usize;
    while hl < need {
        hl += 64;
    }
    let pad = hl - need;

    let mut header = dict.into_bytes();
    header.extend(std::iter::repeat_n(b' ', pad));
    header.push(b'\n');
    // header.len() == hl and (10 + hl) % 64 == 0 by construction.

    w.write_all(&NPY_MAGIC)?;
    w.write_all(&[1, 0])?; // version 1.0
    w.write_all(&(hl as u16).to_le_bytes())?;
    w.write_all(&header)?;
    w.write_all(data)?;
    w.flush()?;
    Ok(())
}

/// Writes a `&[f32]` tensor with little-endian `<f4` and the given `shape`.
pub fn f32_npy<W: Write>(w: &mut W, shape: &[usize], data: &[f32]) -> io::Result<()> {
    let bytes = unsafe {
        // SAFETY: transmuting &[f32] to &[u8] of exactly 4x the length is the
        // standard way to get the raw little-endian representation; f32 has no
        // padding and a fixed 4-byte layout.
        std::slice::from_raw_parts(data.as_ptr() as *const u8, data.len() * 4)
    };
    write_npy(w, "<f4", shape, bytes)
}

/// Writes a `&[f64]` tensor with little-endian `<f8` and the given `shape`.
pub fn f64_npy<W: Write>(w: &mut W, shape: &[usize], data: &[f64]) -> io::Result<()> {
    let bytes = unsafe {
        // SAFETY: see f32_npy; f64 is 8 bytes, no padding.
        std::slice::from_raw_parts(data.as_ptr() as *const u8, data.len() * 8)
    };
    write_npy(w, "<f8", shape, bytes)
}

/// A parsed view into a `.npy` buffer (borrows the underlying bytes).
#[derive(Debug)]
pub struct NpyView<'a> {
    /// Dtype description string (e.g. `"<f4"`).
    pub descr: String,
    /// Tensor shape.
    pub shape: Vec<usize>,
    /// Raw tensor bytes (little-endian, row-major).
    pub data: &'a [u8],
}

/// Parses a `.npy` buffer into its metadata and a borrowed data slice.
pub fn read_npy(bytes: &[u8]) -> Result<NpyView<'_>, NpyError> {
    if bytes.len() < 10 {
        return Err(NpyError::TooShort);
    }
    if bytes[0..6] != NPY_MAGIC {
        return Err(NpyError::BadMagic);
    }
    let hl = u16::from_le_bytes([bytes[8], bytes[9]]) as usize;
    if 10 + hl > bytes.len() {
        return Err(NpyError::BadHeaderLen);
    }
    let header = std::str::from_utf8(&bytes[10..10 + hl]).map_err(|_| NpyError::BadHeader)?;

    // `{'descr': '<f4', ...}` → split on `'` yields tokens [ `{`, `descr`,
    // `: `, `<f4`, ... ]; the *value* is the 4th token (index 3).
    let descr = header
        .split('\'')
        .nth(3)
        .ok_or(NpyError::BadHeader)?
        .to_string();

    let shape_start = header.find("'shape': (").ok_or(NpyError::BadHeader)? + "'shape': (".len();
    let shape_end = header[shape_start..].find(')').ok_or(NpyError::BadHeader)? + shape_start;
    let shape_str = &header[shape_start..shape_end];
    let shape = if shape_str.trim().is_empty() {
        Vec::new()
    } else {
        shape_str
            .split(',')
            .map(|s| s.trim().parse::<usize>())
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| NpyError::BadHeader)?
    };

    let data = &bytes[10 + hl..];
    Ok(NpyView { descr, shape, data })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f32_round_trip_parses_back() {
        let matrix: Vec<f32> = (0..12).map(|i| i as f32).collect();
        let mut buf = Vec::new();
        f32_npy(&mut buf, &[3, 4], &matrix).unwrap();

        let view = read_npy(&buf).unwrap();
        assert_eq!(view.descr, "<f4");
        assert_eq!(view.shape, vec![3, 4]);
        assert_eq!(view.data.len(), 12 * 4);

        // Reinterpret as f32 and check values survive little-endian round-trip.
        let back: Vec<f32> = view
            .data
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect();
        assert_eq!(back, matrix);
    }

    #[test]
    fn header_is_64_byte_aligned() {
        let data: Vec<f32> = vec![1.0, 2.0, 3.0];
        let mut buf = Vec::new();
        f32_npy(&mut buf, &[3], &data).unwrap();
        // prefix(10) + header_len must be a multiple of 64.
        let hl = u16::from_le_bytes([buf[8], buf[9]]) as usize;
        assert_eq!((10 + hl) % 64, 0);
    }
}
