//! Physics-backed simulation fixtures (Phase 4).

pub mod drone;
pub mod world;

pub use drone::QuadDrone;
pub use world::{BodyId, RigidState, World};

/// Fixed control/simulation timestep used by all fixtures (200 Hz).
pub const DT_S: f64 = 0.005;
