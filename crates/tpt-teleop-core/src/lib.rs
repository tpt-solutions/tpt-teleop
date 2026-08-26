//! Central state machine, autonomy handover logic, custom lock-free message
//! bus, platform event loops, thread pinning, and zero-copy wire types.
//!
//! # Module map
//!
//! * [`mode`] / [`machine`] — Auto/Assist/FullTeleop/EmergencyStop modes and
//!   the atomic transition engine (spec §5.4 autonomy handover).
//! * [`mpmc`] — Vyukov-style bounded lock-free MPMC queue.
//! * [`bus`] — fan-out message bus built on tpt-teleop-ring SPSC queues.
//! * [`pool`] — pre-allocated buffer pool; zero steady-state allocation.
//! * [`ser`] — rkyv-derived wire types (`ControlCommand`, telemetry) and
//!   zero-copy serialize/deserialize helpers (spec §3.3).
//! * [`eventloop`] — platform event loops: io_uring / kqueue / IOCP.
//! * [`affinity`] — thread-per-core pinning on Linux/macOS/Windows.
//! * [`profile`] — CPU core-pinning role profiles and their config format.
#![warn(missing_debug_implementations)]

pub mod affinity;
pub mod bus;
pub mod eventloop;
pub mod machine;
pub mod mode;
pub mod mpmc;
pub mod pool;
pub mod prelude;
pub mod profile;
pub mod ser;

pub use machine::StateMachine;
pub use mode::{Mode, ModeError, Transition, TransitionTable};

/// Crate version (from Cargo metadata).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
