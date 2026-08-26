//! CAN bus abstraction; the virtual backend lives in [`crate::mock::can`].

use crate::types::{CanFrame, HalError};

/// Bidirectional CAN endpoint.
///
/// Implementations map to real hardware (SocketCAN / memory-mapped CAN IP
/// cores — Phase 9) or the virtual bus ([`crate::mock::can::CanEndpoint`]).
/// `send`/`recv` never allocate and never block.
pub trait CanBus: Send {
    /// Transmits one frame. `Err(HalError::Dropped)` when the TX queue is
    /// full or fault injection drops it; the frame is handed back by value
    /// semantics (caller retains ownership on error).
    fn send(&mut self, frame: &CanFrame) -> Result<(), HalError>;

    /// Receives into `out`; returns `false` when nothing is pending.
    fn recv(&mut self, out: &mut CanFrame) -> bool;
}

/// Well-known CAN ids used by the built-in mock device fleet.
pub mod ids {
    /// Motor command broadcast (payload: 4 × u16 thrust, big-endian).
    pub const MOTOR_CMD: u32 = 0x200;
    /// Motor telemetry stream base id (+ motor index).
    pub const MOTOR_TELEM: u32 = 0x300;
    /// IMU data stream.
    pub const IMU_DATA: u32 = 0x400;
}
