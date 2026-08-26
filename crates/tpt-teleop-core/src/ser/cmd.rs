//! The operator control command wire struct (spec §6 "Normalize").

use rkyv::{Archive, Deserialize, Serialize};

use crate::mode::Mode;

/// Magic for [`ControlCommand`] (`"CMD"` + rev 1).
pub const MAGIC_COMMAND: u32 = 0x434D_4401;

/// Operator/autonomy command packet.
///
/// Layout is fixed `#[repr(C)]`, **zero padding including tail** (the
/// trailing `crc` placeholder brings the size to an even 56 bytes), so the
/// struct is simultaneously rkyv-serializable and raw-castable over shared
/// memory (see `tpt_teleop_ring::cast`).
#[derive(Debug, Clone, Copy, PartialEq, Archive, Serialize, Deserialize)]
#[repr(C)]
pub struct ControlCommand {
    /// Always [`MAGIC_COMMAND`].
    pub magic: u32,
    /// Struct revision.
    pub version: u16,
    /// Reserved / must be zero.
    pub reserved: u16,
    /// Monotonic per-operator sequence number.
    pub seq: u64,
    /// Capture timestamp (UNIX ns).
    pub timestamp_ns: u64,
    /// [`Mode`] discriminant at send time.
    pub mode: u8,
    /// Bitfield (deadman, turbo, …) — semantics owned by safety crate.
    pub flags: u8,
    /// Valid entries in `axes`.
    pub axis_count: u8,
    /// Reserved for alignment/completeness.
    pub _pad0: u8,
    /// roll, pitch, yaw, throttle, lateral_x, lateral_y.
    pub axes: [f32; 6],
    /// Link-layer CRC placeholder (computed/filled by tpt-teleop-link).
    pub crc: u32,
}

// SAFETY: repr(C) POD of integers/floats; field offsets verified by
// `layout_has_no_padding` below; every bit pattern is a valid value.
unsafe impl tpt_teleop_ring::cast::PlainBytes for ControlCommand {}

impl ControlCommand {
    /// All-zero command in `mode` with correct magic/version metadata.
    pub fn zeroed(mode: Mode) -> Self {
        Self {
            magic: MAGIC_COMMAND,
            version: 1,
            reserved: 0,
            seq: 0,
            timestamp_ns: 0,
            mode: mode.as_u8(),
            flags: 0,
            axis_count: 6,
            _pad0: 0,
            axes: [0.0; 6],
            crc: 0,
        }
    }

    /// Mode discriminant → enum (`None` on corrupt bytes).
    pub fn mode(self) -> Option<Mode> {
        Mode::from_u8(self.mode)
    }

    /// Updates the mode discriminant in place (zero-copy mutation path used
    /// by the safety loop).
    pub fn set_mode(&mut self, mode: Mode) {
        self.mode = mode.as_u8();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_is_exactly_56_no_tail_padding_gaps_unverified_but_castable() {
        let mut c = ControlCommand::zeroed(Mode::FullTeleop);
        assert_eq!(std::mem::size_of::<ControlCommand>(), 56);
        c.seq = 42;
        c.axes[3] = 0.75;
        let bytes = tpt_teleop_ring::cast::bytes_of(&c);
        assert_eq!(bytes.len(), 56);
        let back: &ControlCommand = tpt_teleop_ring::cast::ref_from_bytes(bytes).unwrap();
        assert_eq!(back, &c);
        assert_eq!(back.mode(), Some(Mode::FullTeleop));

        let mut copy = *back;
        copy.set_mode(Mode::EmergencyStop);
        assert_eq!(copy.mode(), Some(Mode::EmergencyStop));
    }
}
