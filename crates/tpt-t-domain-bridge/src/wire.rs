//! Wire structs exchanged between a domain adapter and tpt-teleop.
//!
//! All types derive rkyv `Archive`, so they serialize zero-copy with the same
//! toolchain as `ControlCommand`. Variable-length payloads (frames, clouds,
//! audio) use owned `Vec`s because they are produced on the sensor path, not
//! the control path. The control path (`ControlCommand`) stays fixed-size.

use rkyv::{Archive, Deserialize, Serialize};

/// Pose in 3D space: position in metres, orientation as a unit quaternion
/// `[x, y, z, w]`.
#[derive(Debug, Clone, Copy, PartialEq, Archive, Serialize, Deserialize)]
pub struct Pose6DOF {
    /// Position `[x, y, z]` in metres, in the domain's world frame.
    pub position: [f32; 3],
    /// Orientation quaternion `[x, y, z, w]`.
    pub orientation: [f32; 4],
}

impl Pose6DOF {
    /// Origin pose with identity orientation.
    pub const IDENTITY: Self = Self {
        position: [0.0; 3],
        orientation: [0.0, 0.0, 0.0, 1.0],
    };
}

/// Fault bit for a failing sensor.
pub const FAULT_SENSOR: u32 = 1 << 0;
/// Fault bit for a failing actuator or drive.
pub const FAULT_ACTUATOR: u32 = 1 << 1;
/// Fault bit for an over-temperature condition.
pub const FAULT_OVER_TEMP: u32 = 1 << 2;
/// Fault bit for a geofence or operating-envelope violation.
pub const FAULT_GEOFENCE: u32 = 1 << 3;
/// Fault bit for low battery.
pub const FAULT_LOW_BATTERY: u32 = 1 << 4;

/// The domain's own health and position, as reported to tpt-teleop.
#[derive(Debug, Clone, Copy, PartialEq, Archive, Serialize, Deserialize)]
pub struct DomainState {
    /// Capture time (UNIX microseconds).
    pub timestamp_us: u64,
    /// Current pose estimate.
    pub pose: Pose6DOF,
    /// State of charge, 0.0 to 100.0.
    pub battery_pct: f32,
    /// Bitwise OR of the `FAULT_*` constants currently raised.
    pub fault_flags: u32,
}

impl DomainState {
    /// True when no fault bit is set.
    pub const fn is_healthy(&self) -> bool {
        self.fault_flags == 0
    }
}

/// Telemetry as carried inside a [`SensorFeed`]. Same content as
/// [`DomainState`], kept as a separate name to match the spec's `SensorFeed`
/// field.
pub type DomainTelemetry = DomainState;

/// Kind of video stream a frame came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Archive, Serialize, Deserialize)]
#[repr(u8)]
pub enum VideoKind {
    /// Visible-light RGB.
    Rgb = 0,
    /// Thermal (infrared) imager.
    Thermal = 1,
    /// Depth map.
    Depth = 2,
}

/// One video frame. `data` is row-major, tightly packed, with no row padding.
#[derive(Debug, Clone, PartialEq, Archive, Serialize, Deserialize)]
pub struct VideoFrame {
    /// Capture time (UNIX microseconds).
    pub timestamp_us: u64,
    /// Which stream this frame belongs to.
    pub kind: VideoKind,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Pixel bytes, packed.
    pub data: Vec<u8>,
}

/// One LiDAR or radar sweep.
#[derive(Debug, Clone, PartialEq, Archive, Serialize, Deserialize)]
pub struct PointCloud {
    /// Capture time (UNIX microseconds).
    pub timestamp_us: u64,
    /// Points `[x, y, z]` in the sensor frame, metres.
    pub points: Vec<[f32; 3]>,
}

/// Force/torque feedback for the operator's haptic device.
#[derive(Debug, Clone, Copy, PartialEq, Archive, Serialize, Deserialize)]
pub struct HapticFeedback {
    /// Force `[fx, fy, fz]` in newtons and torque `[tx, ty, tz]` in newton-metres.
    pub force_torque: [f32; 6],
}

/// Microphone audio from the robot.
#[derive(Debug, Clone, PartialEq, Archive, Serialize, Deserialize)]
pub struct AudioStream {
    /// Capture time (UNIX microseconds) of the first sample.
    pub timestamp_us: u64,
    /// Sample rate in hertz.
    pub sample_rate_hz: u32,
    /// Interleaved channel count.
    pub channels: u16,
    /// Interleaved 16-bit PCM samples.
    pub samples: Vec<i16>,
}

/// Everything a domain streams to the operator in one tick.
#[derive(Debug, Clone, PartialEq, Archive, Serialize, Deserialize)]
pub struct SensorFeed {
    /// Tick time (UNIX microseconds).
    pub timestamp_us: u64,
    /// Video frames (RGB, thermal, depth), in any order.
    pub video_frames: Vec<VideoFrame>,
    /// Point clouds (LiDAR, radar).
    pub point_clouds: Vec<PointCloud>,
    /// Domain telemetry snapshot.
    pub telemetry: DomainTelemetry,
    /// Haptic feedback for the operator device.
    pub haptic_data: HapticFeedback,
    /// Microphone audio, if the robot has a microphone.
    pub audio: Option<AudioStream>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_feed() -> SensorFeed {
        SensorFeed {
            timestamp_us: 1_700_000_000_000_000,
            video_frames: vec![VideoFrame {
                timestamp_us: 1,
                kind: VideoKind::Thermal,
                width: 2,
                height: 2,
                data: vec![1, 2, 3, 4],
            }],
            point_clouds: vec![PointCloud {
                timestamp_us: 2,
                points: vec![[1.0, 2.0, 3.0]],
            }],
            telemetry: DomainState {
                timestamp_us: 3,
                pose: Pose6DOF::IDENTITY,
                battery_pct: 87.5,
                fault_flags: FAULT_OVER_TEMP,
            },
            haptic_data: HapticFeedback {
                force_torque: [0.5, 0.0, -0.5, 0.0, 0.0, 0.1],
            },
            audio: Some(AudioStream {
                timestamp_us: 4,
                sample_rate_hz: 16_000,
                channels: 1,
                samples: vec![-1, 0, 1],
            }),
        }
    }

    #[test]
    fn sensor_feed_round_trips_through_rkyv() {
        let feed = sample_feed();
        let bytes = rkyv::to_bytes::<rkyv::rancor::Error>(&feed).unwrap();
        let back: SensorFeed = rkyv::from_bytes::<SensorFeed, rkyv::rancor::Error>(&bytes).unwrap();
        assert_eq!(back, feed);
    }

    #[test]
    fn health_follows_fault_flags() {
        let mut s = sample_feed().telemetry;
        assert!(!s.is_healthy());
        s.fault_flags = 0;
        assert!(s.is_healthy());
    }
}
