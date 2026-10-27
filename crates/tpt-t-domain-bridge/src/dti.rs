//! The [`DomainTeleopInterface`] trait: the contract every domain adapter
//! implements to plug a robot into tpt-teleop.

use core::fmt;

use rkyv::{Archive, Deserialize, Serialize};
use tpt_t_core::ser::ControlCommand;

use crate::wire::{DomainState, SensorFeed};

/// Identifies the operator (human, or AI agent) holding control of a session.
///
/// Sixteen opaque bytes, UUID-shaped. The core tracks the operator per session,
/// not per command, so the id is supplied once on engage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Archive, Serialize, Deserialize)]
#[repr(transparent)]
pub struct OperatorId(pub [u8; 16]);

impl OperatorId {
    /// Wraps raw id bytes.
    pub const fn new(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }

    /// The raw id bytes.
    pub const fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }
}

/// Errors surfaced by the DTI and by [`DomainSession`](crate::DomainSession).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DomainError {
    /// The lifecycle does not permit `from → to`.
    IllegalTransition {
        /// State the session was in.
        from: crate::HandoverState,
        /// State that was requested.
        to: crate::HandoverState,
    },
    /// A control command arrived while no operator holds the unit.
    NotEngaged {
        /// State the session was in when the command arrived.
        state: crate::HandoverState,
    },
    /// The domain adapter failed to carry out a request.
    Adapter(String),
}

impl DomainError {
    /// Convenience constructor for adapter implementations.
    pub fn adapter(msg: impl Into<String>) -> Self {
        Self::Adapter(msg.into())
    }
}

impl fmt::Display for DomainError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DomainError::IllegalTransition { from, to } => write!(
                f,
                "handover transition {} -> {} is not permitted",
                from.name(),
                to.name()
            ),
            DomainError::NotEngaged { state } => write!(
                f,
                "control command rejected: session is {}, not TELEOP_ACTIVE",
                state.name()
            ),
            DomainError::Adapter(msg) => write!(f, "domain adapter error: {msg}"),
        }
    }
}

impl std::error::Error for DomainError {}

/// Result alias used throughout the DTI.
pub type Result<T> = core::result::Result<T, DomainError>;

/// The integration contract a domain adapter implements.
///
/// The adapter owns all domain physics and safety rules. tpt-teleop owns
/// transport, arbitration and the handover lifecycle. Calls arrive through a
/// [`DomainSession`](crate::DomainSession), which rejects anything that is out
/// of order for the current [`HandoverState`](crate::HandoverState), so an
/// adapter can assume it is only engaged while a human holds the unit.
pub trait DomainTeleopInterface {
    /// Called when an operator takes control. The adapter should begin
    /// enforcing teleop-mode limits (speed, geofence) from this point.
    fn on_teleop_engage(&mut self, operator: OperatorId) -> Result<()>;

    /// Called when control is released. The adapter should ramp its outputs
    /// down to a safe state and prepare to hand back to autonomy without a jerk.
    fn on_teleop_disengage(&mut self) -> Result<()>;

    /// Receives one normalized control command from the operator.
    fn on_control_command(&mut self, cmd: ControlCommand) -> Result<()>;

    /// Reports the domain's current state (pose, battery, faults) to tpt-teleop.
    fn get_domain_state(&self) -> DomainState;

    /// Supplies sensor data to stream to the operator (video, point clouds,
    /// telemetry, haptics, audio).
    fn get_sensor_feed(&mut self) -> SensorFeed;

    /// Called on any emergency stop, from either side. An E-stop is never
    /// refused, so this is infallible and the default does nothing. Override it
    /// to cut actuators immediately.
    fn on_emergency_stop(&mut self) {}
}
