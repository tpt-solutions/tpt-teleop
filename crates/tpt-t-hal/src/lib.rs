//! Hardware abstraction layer: motors, sensors, CAN bus, cameras.
//!
//! Ships a full mock backend so the entire stack runs without hardware:
//! a virtual CAN bus over SPSC rings ([`mock::can`]), deterministic ESC
//! models ([`mock::motor`]), a null camera ([`mock::camera`]), and a physics
//! quadrotor fixture ([`sim::QuadDrone`]) for end-to-end integration tests.
//! Real SocketCAN / MAVLink / V4L2 backends land with Phase 9 behind these
//! same traits.

pub mod camera;
pub mod can;
pub mod mavlink;
pub mod mmio;
pub mod mock;
pub mod motor;
pub mod sensor;
pub mod sim;
pub mod stub_can;
pub mod types;

#[cfg(target_os = "linux")]
pub mod socketcan;

pub use camera::Camera;
pub use can::{CanBus, ids};
pub use mavlink::{
    Attitude, Heartbeat, MavError, MavFrame, MavParser, decode_attitude, decode_heartbeat,
    mav_crc16, parse_frame,
};
pub use mock::can_pair;
pub use motor::Motor;
pub use sensor::{GpsSource, ImuSource, TelemetrySource};
pub use sim::{QuadDrone, World};
pub use stub_can::StubCan;
pub use types::{
    CanFrame, FrameInfo, HalError, MotorCommand, MotorMode, MotorTelemetry, PixelFormat, Pose6D,
};

#[cfg(target_os = "linux")]
pub use mmio::LinuxMmio;
pub use mmio::{BufferMmio, Mmio};
#[cfg(target_os = "linux")]
pub use socketcan::SocketCan;

/// Crate version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
