//! Telemetry wire structs (IMU, GPS, generic multi-value packets).
//!
//! All structs follow the workspace conventions from [`crate::ser`]:
//! rkyv-derived, `#[repr(C)]`, zero interior/tail padding (asserted by
//! tests), therefore both rkyv- and raw-cast-friendly.

use rkyv::{Archive, Deserialize, Serialize};

/// Magic shared base for all telemetry structs (`"TLM"`).
pub const MAGIC_TELEMETRY: u32 = 0x544C_4D00;

/// What kind of sample a [`TelemetryPacket`] carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum TelemetryKind {
    /// Generic float vector.
    Generic = 0,
    /// Battery/state-of-charge values.
    Battery = 1,
    /// Pose estimate (x,y,z,yaw,pitch,roll,…).
    Pose = 2,
    /// CPU/encoder temperatures.
    Temperature = 3,
}

impl TelemetryKind {
    /// Discriminant.
    pub fn as_u16(self) -> u16 {
        self as u16
    }

    /// Inverse of [`as_u16`](Self::as_u16).
    pub fn from_u16(v: u16) -> Option<Self> {
        match v {
            0 => Some(Self::Generic),
            1 => Some(Self::Battery),
            2 => Some(Self::Pose),
            3 => Some(Self::Temperature),
            _ => None,
        }
    }
}

/// 6-axis inertial sample — 48 bytes dense, no padding.
#[derive(Debug, Clone, Copy, PartialEq, Archive, Serialize, Deserialize)]
#[repr(C)]
pub struct ImuSample {
    /// [`ImuSample::MAGIC`].
    pub magic: u32,
    /// Revision.
    pub version: u16,
    /// Reserved.
    pub reserved: u16,
    /// Monotonic sequence.
    pub seq: u64,
    /// UNIX ns stamp.
    pub timestamp_ns: u64,
    /// Gyro rad/s (x,y,z).
    pub gyro_rps: [f32; 3],
    /// Accel g (x,y,z).
    pub accel_g: [f32; 3],
}
// SAFETY: repr(C) dense primitives only; `layouts_are_dense_and_castable`
// proves size matches field-sum so no hidden padding exists.
unsafe impl tpt_t_ring::cast::PlainBytes for ImuSample {}

impl ImuSample {
    /// Magic: telemetry base | 1.
    pub const MAGIC: u32 = MAGIC_TELEMETRY | 1;

    /// Fresh zero sample stamped with `seq`/`ts`.
    pub fn zeroed(seq: u64, timestamp_ns: u64) -> Self {
        Self {
            magic: Self::MAGIC,
            version: 1,
            reserved: 0,
            seq,
            timestamp_ns,
            gyro_rps: [0.0; 3],
            accel_g: [0.0; 3],
        }
    }
}

/// GNSS fix — 64 bytes dense, no padding.
#[derive(Debug, Clone, Copy, PartialEq, Archive, Serialize, Deserialize)]
#[repr(C)]
pub struct GpsSample {
    /// [`GpsSample::MAGIC`].
    pub magic: u32,
    /// Revision.
    pub version: u16,
    /// Reserved.
    pub reserved: u16,
    /// Monotonic sequence.
    pub seq: u64,
    /// UNIX ns stamp.
    pub timestamp_ns: u64,
    /// Degrees north.
    pub lat_deg: f64,
    /// Degrees east.
    pub lon_deg: f64,
    /// Meters above MSL.
    pub alt_m: f64,
    /// Meters/second ground speed.
    pub speed_mps: f32,
    /// Degrees true north.
    pub course_deg: f32,
    /// Satellites used in fix.
    pub sats: u32,
    /// Fix-quality flag (1 = good).
    pub fix_ok: u32,
}
// SAFETY: repr(C) dense primitives; size asserted == field sum below.
unsafe impl tpt_t_ring::cast::PlainBytes for GpsSample {}

impl GpsSample {
    /// Magic: telemetry base | 2.
    pub const MAGIC: u32 = MAGIC_TELEMETRY | 2;

    /// Fresh zero fix stamped with `seq`/`ts`.
    pub fn zeroed(seq: u64, timestamp_ns: u64) -> Self {
        Self {
            magic: Self::MAGIC,
            version: 1,
            reserved: 0,
            seq,
            timestamp_ns,
            lat_deg: 0.0,
            lon_deg: 0.0,
            alt_m: 0.0,
            speed_mps: 0.0,
            course_deg: 0.0,
            sats: 0,
            fix_ok: 0,
        }
    }
}

/// Generic N-value telemetry frame — 56 bytes dense, no padding.
#[derive(Debug, Clone, Copy, PartialEq, Archive, Serialize, Deserialize)]
#[repr(C)]
pub struct TelemetryPacket {
    /// [`TelemetryPacket::MAGIC`].
    pub magic: u32,
    /// [`TelemetryKind`] discriminant.
    pub kind: u16,
    /// Reserved.
    pub reserved: u16,
    /// Monotonic sequence.
    pub seq: u64,
    /// UNIX ns stamp.
    pub timestamp_ns: u64,
    /// Payload values; unused slots are zeros.
    pub values: [f32; 8],
}
// SAFETY: repr(C) dense primitives.
unsafe impl tpt_t_ring::cast::PlainBytes for TelemetryPacket {}

impl TelemetryPacket {
    /// Magic: telemetry base | 0.
    pub const MAGIC: u32 = MAGIC_TELEMETRY;

    /// Fresh empty packet of `kind`.
    pub fn zeroed(kind: TelemetryKind, seq: u64, timestamp_ns: u64) -> Self {
        Self {
            magic: Self::MAGIC,
            kind: kind.as_u16(),
            reserved: 0,
            seq,
            timestamp_ns,
            values: [0.0; 8],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tpt_t_ring::cast::{bytes_of, ref_from_bytes};

    #[test]
    fn layouts_are_dense_and_castable() {
        assert_eq!(std::mem::size_of::<ImuSample>(), 48);
        assert_eq!(std::mem::size_of::<GpsSample>(), 64);
        assert_eq!(std::mem::size_of::<TelemetryPacket>(), 56);

        let imu = ImuSample {
            gyro_rps: [0.1; 3],
            ..ImuSample::zeroed(1, 2)
        };
        assert_eq!(*ref_from_bytes::<ImuSample>(bytes_of(&imu)).unwrap(), imu);

        let gps = GpsSample {
            lat_deg: 47.6,
            ..GpsSample::zeroed(3, 4)
        };
        assert_eq!(*ref_from_bytes::<GpsSample>(bytes_of(&gps)).unwrap(), gps);

        let pkt = TelemetryPacket {
            values: [0.5; 8],
            ..TelemetryPacket::zeroed(TelemetryKind::Pose, 5, 6)
        };
        let back: &TelemetryPacket = ref_from_bytes(bytes_of(&pkt)).unwrap();
        assert_eq!(*back, pkt);
        assert_eq!(
            TelemetryKind::from_u16(back.kind),
            Some(TelemetryKind::Pose)
        );
    }
}
