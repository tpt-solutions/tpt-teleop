//! Zero-copy serialization conventions and helpers (spec §3.3).
//!
//! # Conventions (workspace-wide)
//!
//! 1. Every wire struct derives `rkyv::{Archive, Serialize, Deserialize}` and
//!    is `#[repr(C)]` plain-old-data with **no padding** (tail padding
//!    included) so it also supports raw casting via `tpt_t_ring::cast`
//!    for same-machine shared-memory paths.
//! 2. Archived buffers are aligned to [`WIRE_ALIGN`] (one cache line).
//! 3. Every packet starts with a [`WireFrame`] header (magic + version +
//!    payload length) so link-layer code can validate cheaply.
//! 4. Serialization targets pre-allocated buffers only — see
//!    [`serialize_into`], which serializes directly into caller-owned memory
//!    and reports the payload length. No heap allocation occurs.

pub mod cmd;
pub mod telemetry;

pub use cmd::{ControlCommand, FLAG_AI_ORIGIN};
pub use telemetry::{GpsSample, ImuSample, TelemetryKind, TelemetryPacket};

use rkyv::{
    Archive, Portable, Serialize,
    api::high::{HighSerializer, HighValidator, to_bytes_in},
    bytecheck::CheckBytes,
    ser::allocator::ArenaHandle,
    util::AlignedVec,
};

/// Alignment every serialized wire buffer honors.
pub const WIRE_ALIGN: usize = 64;

/// Pre-aligned growable wire buffer type (reuses its allocation).
pub type AlignedBuf = AlignedVec<WIRE_ALIGN>;

/// Default wire error type (`rancor` boxed error).
pub type WireError = rkyv::rancor::BoxedError;

/// Magic marker `"TPT1"` starting every frame.
pub const FRAME_MAGIC: u32 = 0x5450_5431;

/// Current protocol revision.
pub const PROTOCOL_VERSION: u16 = 1;

/// Link-layer frame header prepended to every payload (also POD-castable).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub struct WireFrame {
    /// Always [`FRAME_MAGIC`]; receivers reject anything else.
    pub magic: u32,
    /// Protocol revision ([`PROTOCOL_VERSION`]).
    pub version: u16,
    /// Payload byte count following this header.
    pub payload_len: u16,
}
// SAFETY: repr(C); 4+2+2 packs densely with no padding; any bits valid.
unsafe impl tpt_t_ring::cast::PlainBytes for WireFrame {}

/// Serializes `value` **directly into** the caller's pre-allocated buffer,
/// reusing its backing allocation. Returns the number of bytes written.
///
/// This is the spec §6 "Serialize" step: no intermediate allocation between
/// struct and wire bytes.
pub fn serialize_into<'a, T>(value: &T, buf: &mut AlignedBuf) -> Result<usize, WireError>
where
    T: for<'b> Serialize<HighSerializer<AlignedBuf, ArenaHandle<'b>, WireError>>,
{
    let recycled = std::mem::take(buf);
    let mut recycled = recycled;
    recycled.clear(); // append-mode writer: reset length, keep allocation
    *buf = to_bytes_in::<_, WireError>(value, recycled)?;
    Ok(buf.len())
}

/// Validates and returns a zero-copy reference to the archived form of `T`.
///
/// Full deserialization back to owned values is intentionally not provided
/// here: every wire type in this workspace is plain-old-data, so hot-path
/// readers either inspect the archived form (`access_root`) or raw-cast the
/// payload via `tpt_t_ring::cast` — both are zero-copy by construction.
pub fn access_root<'a, T>(bytes: &'a [u8]) -> Result<&'a <T as Archive>::Archived, WireError>
where
    T: Archive,
    <T as Archive>::Archived: Portable + for<'b> CheckBytes<HighValidator<'b, WireError>>,
{
    rkyv::access(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mode::Mode;

    #[test]
    fn frame_header_cast_roundtrip() {
        let f = WireFrame {
            magic: FRAME_MAGIC,
            version: PROTOCOL_VERSION,
            payload_len: 42,
        };
        assert_eq!(std::mem::size_of::<WireFrame>(), 8);
        let bytes = tpt_t_ring::cast::bytes_of(&f);
        let (back, rest): (&WireFrame, &[u8]) = tpt_t_ring::cast::split_prefix(bytes).unwrap();
        assert_eq!(back.magic, f.magic);
        assert!(rest.is_empty());
    }

    #[test]
    fn control_command_rkyv_roundtrip_into_prealigned_buffer() {
        let mut buf = AlignedBuf::new();
        let cmd = ControlCommand {
            seq: 7,
            timestamp_ns: 123_456,
            mode: Mode::FullTeleop.as_u8(),
            flags: 0,
            axes: [0.1, 0.2, 0.3, 0.4, 0.5, 0.6],
            ..ControlCommand::zeroed(Mode::FullTeleop)
        };
        let n = serialize_into(&cmd, &mut buf).unwrap();
        assert!(n > 0 && n <= buf.len());
        assert_eq!(buf.as_ptr() as usize % WIRE_ALIGN, 0);

        let arch = access_root::<ControlCommand>(&buf[..n]).unwrap();
        assert_eq!(arch.seq, 7);
        assert_eq!(arch.mode, Mode::FullTeleop as u8);
        assert_eq!(arch.timestamp_ns, 123_456);
        assert_eq!(arch.axes[3], 0.4);

        // Second serialize reuses the same allocation (no growth needed).
        let ptr_before = buf.as_ptr();
        let _ = serialize_into(&cmd, &mut buf).unwrap();
        assert_eq!(buf.as_ptr(), ptr_before, "buffer must be reused");
    }
}
