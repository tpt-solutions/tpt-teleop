//! Deterministic RT safety loop: geofencing, predictive collision avoidance
//! (kinematic limit enforcement), spline-smoothed autonomy handover, latency
//! compensation, and unconditional emergency overrides on a dedicated RT
//! thread.
//!
//! Hot path contract (spec §5.4 / §6): pop from the input ring → mutate in
//! place through the pipeline → push to the output ring — zero allocations,
//! zero locks.

pub mod geo;
pub mod latency;
pub mod limits;
pub mod loop_;
pub mod rt;
pub mod spline;
pub mod veto;

pub use geo::{FenceVerdict, GeoFence, axis};
pub use latency::LatencyCompensator;
pub use limits::{KinematicLimits, write_emergency_stop};
pub use loop_::{InterceptStats, SafetyConfig, SafetyLoop, SafetyThreadHandle};
pub use rt::{RtError, elevate_current_thread};
pub use spline::{AuthorityBlend, authority_target, smootherstep};
pub use veto::VetoGate;

/// Crate version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
