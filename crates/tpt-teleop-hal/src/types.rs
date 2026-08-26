//! Shared plain-old-data types crossing the HAL boundary.
//!
//! Everything here is `#[repr(C)]`, padding-free, `Copy`, and
//! [`PlainBytes`](tpt_teleop_ring::cast::PlainBytes), so device drivers move
//! records through SPSC rings and shared memory with zero copies — the same
//! discipline as `tpt-teleop-core::ser`.

use tpt_teleop_ring::cast::PlainBytes;

/// Magic for [`CanFrame`] ("CAN1").
pub const MAGIC_CAN_FRAME: u32 = 0x4341_4E31;

/// One classical-CAN 2.0 frame: 11/29-bit id plus up to 8 payload bytes.
/// 16 bytes dense, no padding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(C)]
pub struct CanFrame {
    /// CAN identifier (11-bit standard or full 29-bit extended).
    pub id: u32,
    /// Payload length in bytes (0–8).
    pub len: u16,
    /// Payload bytes; only [`len`](Self::len) are valid.
    pub data: [u8; 8],
    /// Reserved / must be zero.
    pub _reserved: u16,
}
// SAFETY: repr(C) POD; 4+2+8+2 = 16 bytes exactly, dense.
unsafe impl PlainBytes for CanFrame {}

impl CanFrame {
    /// Frame header magic constant.
    pub const MAGIC: u32 = MAGIC_CAN_FRAME;

    /// Builds a frame from a payload slice (`len ≤ 8`; extra bytes ignored).
    pub fn new(id: u32, payload: &[u8]) -> Self {
        let len = payload.len().min(8) as u16;
        let mut data = [0u8; 8];
        data[..len as usize].copy_from_slice(&payload[..len as usize]);
        Self {
            id,
            len,
            data,
            _reserved: 0,
        }
    }

    /// Valid payload slice.
    #[inline]
    pub fn payload(&self) -> &[u8] {
        &self.data[..self.len.min(8) as usize]
    }
}

/// Motor operating mode ([`MotorCommand::mode`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum MotorMode {
    /// Output disabled; rotor coasts.
    Idle = 0,
    /// Closed-loop speed target, `value` = rad/s.
    Speed = 1,
    /// Direct thrust target, `value` = normalized 0..1.
    Thrust = 2,
}

impl MotorMode {
    /// Discriminant.
    #[inline]
    pub fn as_u8(self) -> u8 {
        self as u8
    }

    /// Inverse mapping; `None` on corrupt values.
    #[inline]
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Idle),
            1 => Some(Self::Speed),
            2 => Some(Self::Thrust),
            _ => None,
        }
    }
}

/// Command sent to one motor. 24 bytes dense, no padding.
#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct MotorCommand {
    /// Monotonic command sequence.
    pub seq: u64,
    /// Capture timestamp (UNIX ns).
    pub timestamp_ns: u64,
    /// [`MotorMode`] discriminant.
    pub mode: u8,
    /// Reserved / must be zero.
    pub _reserved: [u8; 3],
    /// Mode-dependent setpoint (rad/s or normalized thrust).
    pub value: f32,
}
// SAFETY: repr(C); 8+8+1+3+4 = 24 exactly, dense primitives only.
unsafe impl PlainBytes for MotorCommand {}

impl MotorCommand {
    /// Idle command stamped with `seq`/`ts`.
    pub fn idle(seq: u64, timestamp_ns: u64) -> Self {
        Self {
            seq,
            timestamp_ns,
            mode: MotorMode::Idle.as_u8(),
            _reserved: [0; 3],
            value: 0.0,
        }
    }

    /// Operating mode (`None` on corrupt bytes).
    pub fn mode(self) -> Option<MotorMode> {
        MotorMode::from_u8(self.mode)
    }
}

/// Read-back state from one motor. 32 bytes dense, no padding.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
#[repr(C)]
pub struct MotorTelemetry {
    /// Last applied command sequence.
    pub seq: u64,
    /// Sample timestamp (UNIX ns).
    pub timestamp_ns: u64,
    /// Measured rotor speed (rad/s).
    pub rpm: f32,
    /// Estimated winding temperature (°C).
    pub temp_c: f32,
    /// Supply voltage (V).
    pub volts: f32,
    /// Driver error bitfield (vendor-specific).
    pub errors: u32,
}
// SAFETY: repr(C) dense primitives; 8+8+4+4+4+4 = 32 exactly.
unsafe impl PlainBytes for MotorTelemetry {}

/// Camera pixel format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum PixelFormat {
    /// 8-bit grayscale.
    GrayY8,
    /// Packed 24-bit RGB.
    Rgb888,
    /// Semi-planar NV12 (Y plane + interleaved UV).
    Nv12,
}

impl PixelFormat {
    /// Bytes per pixel (12/8 = 1.5 for NV12 — see [`min_buffer_len`](Self::min_buffer_len)).
    #[inline]
    pub fn bytes_per_pixel(self) -> u32 {
        match self {
            PixelFormat::GrayY8 => 1,
            PixelFormat::Rgb888 => 3,
            PixelFormat::Nv12 => 2, // averaged; NV12 uses min_buffer_len
        }
    }

    /// Minimum buffer bytes for a `w×h` frame in this format.
    #[inline]
    pub fn min_buffer_len(self, w: u32, h: u32) -> usize {
        match self {
            // NV12: w*h luma + (w/2)*(h/2)*2 interleaved chroma = w*h*3/2.
            PixelFormat::Nv12 => (w as usize) * (h as usize) * 3 / 2,
            other => (other.bytes_per_pixel() as usize) * w as usize * h as usize,
        }
    }
}

/// Metadata returned alongside a grabbed frame. 32 bytes dense.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(C)]
pub struct FrameInfo {
    /// Monotonic frame sequence.
    pub seq: u64,
    /// Capture timestamp (UNIX ns).
    pub timestamp_ns: u64,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Row stride in bytes.
    pub stride: u32,
    /// [`PixelFormat`] discriminant.
    pub format: u32,
}
// SAFETY: repr(C) dense primitives; 8+8+4+4+4+4 = 32 exactly.
unsafe impl PlainBytes for FrameInfo {}

/// 6-DOF pose: meters + radians. 40 bytes dense, no padding.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
#[repr(C)]
pub struct Pose6D {
    /// Position X (m).
    pub x: f64,
    /// Position Y (m).
    pub y: f64,
    /// Position Z (m, up-positive).
    pub z: f64,
    /// Yaw around Z (rad).
    pub yaw: f32,
    /// Pitch around Y (rad).
    pub pitch: f32,
    /// Roll around X (rad).
    pub roll: f32,
    /// Reserved / alignment completeness.
    pub _reserved: u32,
}
// SAFETY: repr(C); 8*3 + 4*3 + 4 = 40 exactly, dense.
unsafe impl PlainBytes for Pose6D {}

/// HAL-level failures shared across device kinds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HalError {
    /// Device rejected the operation (busy/offline); vendor text attached.
    Device(&'static str),
    /// Caller-provided buffer too small.
    BufferTooSmall { needed: usize, got: usize },
    /// Transport dropped the frame (fault injection / congestion).
    Dropped,
    /// No acknowledgment from the device.
    NoAck,
}

impl core::fmt::Display for HalError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            HalError::Device(s) => write!(f, "device error: {s}"),
            HalError::BufferTooSmall { needed, got } => {
                write!(f, "buffer too small: need {needed}, got {got}")
            }
            HalError::Dropped => write!(f, "frame dropped by transport"),
            HalError::NoAck => write!(f, "no acknowledgment from device"),
        }
    }
}

impl std::error::Error for HalError {}
