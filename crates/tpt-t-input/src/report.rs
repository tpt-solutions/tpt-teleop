//! Device identity + controller report records (plain-old-data).

use tpt_t_ring::cast::PlainBytes;

/// Static identity of an opened input device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceInfo {
    /// USB vendor id (0 when unknown).
    pub vendor_id: u16,
    /// USB product id (0 when unknown).
    pub product_id: u16,
    /// OS-specific handle/path (evdev node, HID interface path, …).
    pub path: String,
    /// Number of absolute axes the device exposes (best effort).
    pub num_axes: u8,
    /// Number of buttons the device exposes (best effort, ≤ 64 tracked).
    pub num_buttons: u8,
}

/// Normalized controller snapshot produced every successful
/// [`RawInputSource::poll`](crate::source::RawInputSource::poll).
///
/// Dense plain-old-data (56 bytes, no padding) so reports cross SPSC rings
/// and shared memory with zero copies, exactly like the wire structs in
/// `tpt_t_core::ser`.
#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct ControllerReport {
    /// Monotonic report sequence (wraps at u32; increments per poll hit).
    pub seq: u32,
    /// Button bitfield (bit = button index from the device layout).
    pub buttons: u32,
    /// Absolute axes normalized to [-1, 1] (triggers may use [0, 1]).
    pub axes: [f32; 8],
    /// Capture timestamp (UNIX ns).
    pub timestamp_ns: u64,
}
// SAFETY: repr(C); 4+4+32+8 = 48… actual layout: seq(4) buttons(4) axes(32)
// timestamp(8) = 48 exactly, dense integers/floats only.
unsafe impl PlainBytes for ControllerReport {}

impl Default for ControllerReport {
    fn default() -> Self {
        Self {
            seq: 0,
            buttons: 0,
            axes: [0.0; 8],
            timestamp_ns: 0,
        }
    }
}

/// Semantic channel slots inside [`ControllerReport::axes`].
pub mod slot {
    /// Roll axis (right stick X on typical gamepads).
    pub const ROLL: usize = 0;
    /// Pitch axis (right stick Y).
    pub const PITCH: usize = 1;
    /// Yaw rate axis (left stick X).
    pub const YAW: usize = 2;
    /// Collective/throttle axis (left stick Y or wheel trigger).
    pub const THROTTLE: usize = 3;
    /// Lateral X velocity trim.
    pub const LAT_X: usize = 4;
    /// Lateral Y velocity trim.
    pub const LAT_Y: usize = 5;
    /// Spare analog input.
    pub const SPARE0: usize = 6;
    /// Spare analog input.
    pub const SPARE1: usize = 7;
}
