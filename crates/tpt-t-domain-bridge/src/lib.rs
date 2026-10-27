//! The Domain Teleoperation Interface (DTI): the single integration crate that
//! every domain repo (`tpt-t-agri`, `tpt-t-urban-teleop`, …) depends on.
//!
//! # Module map
//!
//! * [`dti`] — the [`DomainTeleopInterface`] trait a domain adapter implements,
//!   and [`OperatorId`] / [`Result`] / [`DomainError`].
//! * [`wire`] — rkyv-derived sensor/state structs the adapter exchanges with
//!   tpt-teleop (`SensorFeed`, `DomainState`, `Pose6DOF`, …).
//! * [`lifecycle`] — the universal autonomy ↔ teleop handover state machine
//!   and its mapping onto the core [`Mode`](tpt_t_core::Mode).
//! * [`session`] — [`DomainSession`], the driver that enforces the lifecycle in
//!   front of an adapter, and [`TeleopLink`], the `request_teleop_assistance()`
//!   surface a domain calls when autonomy can no longer continue.
//!
//! # Where the spec and the code differ
//!
//! `bridge spec.txt` describes a richer model than tpt-teleop-core implements
//! today. This crate follows the implemented wire format wherever the two
//! disagree, and records the mapping here so adapters have one place to look:
//!
//! | Spec                                   | Implemented                                   |
//! |----------------------------------------|-----------------------------------------------|
//! | `ControlCommand { operator_id, primary_input, secondary_input, buttons, haptic_feedback }` | [`tpt_t_core::ser::ControlCommand`]: flat 56-byte POD (`seq`, `mode`, `flags`, `axes[6]`). Passed to adapters unchanged. |
//! | `operator_id` per command              | Carried once, by [`DomainTeleopInterface::on_teleop_engage`], and held by [`DomainSession`] for the whole session. Commands carry `seq` and `mode`, not an operator id. |
//! | `InputState` / `ButtonState` / `HapticCmd` | Not implemented. Axes live in `axes[..axis_count]`; E-stop is `mode == EmergencyStop`; haptic output goes to the operator via [`wire::HapticFeedback`] in `SensorFeed`. |
//! | `AUTONOMOUS`, `REQUESTING_TELEOP`, `TELEOP_ACTIVE`, `RETURNING_TO_AUTONOMY`, `EMERGENCY_STOP` | [`HandoverState`], which maps onto the core `Mode` (see [`HandoverState::to_mode`]). |
//! | Direct `REQUESTING_TELEOP → TELEOP_ACTIVE` | Allowed here. The core `StateMachine` still stages `Auto → Assist → FullTeleop`, so the adapter sees `engage` only after that staging has completed. |
//! | Shared control (human + AI on one unit) | Arbitrated upstream in tpt-teleop-core/safety. The adapter receives one `ControlCommand` per call, never two streams. |
//!
//! `operator_id` is a 16-byte value, the same shape as a UUID, so a `Uuid` can
//! be converted with `OperatorId::new(uuid.into_bytes())` without this crate
//! taking a dependency on `uuid`.
#![warn(missing_debug_implementations)]

pub mod dti;
pub mod lifecycle;
pub mod session;
pub mod wire;

pub use dti::{DomainError, DomainTeleopInterface, OperatorId, Result};
pub use lifecycle::HandoverState;
pub use session::{AssistanceReason, DomainSession, TeleopLink};
pub use wire::{
    AudioStream, DomainState, DomainTelemetry, HapticFeedback, PointCloud, Pose6DOF, SensorFeed,
    VideoFrame, VideoKind,
};

/// Crate version (from Cargo metadata).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
