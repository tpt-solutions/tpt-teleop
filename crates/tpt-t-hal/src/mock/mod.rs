//! In-process mock devices backing the HAL traits.
//!
//! These exist so the entire control stack runs against simulated hardware
//! (Phase 4 goal): the virtual CAN bus is built from wait-free SPSC rings,
//! and the simulated ESCs expose deterministic first-order dynamics that the
//! physics fixture (`crate::sim`) consumes.

pub mod camera;
pub mod can;
pub mod motor;

pub use camera::NullCamera;
pub use can::{CanEndpoint, can_pair};
pub use motor::SimMotor;
