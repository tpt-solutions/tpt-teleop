//! The handover driver that sits between tpt-teleop and a domain adapter.
//!
//! [`DomainSession`] owns one adapter and enforces the
//! [`HandoverState`] lifecycle in front of it. A call that is out of order for
//! the current state returns an error without reaching the adapter. A failed
//! adapter call leaves the state unchanged. The emergency stop is the one
//! exception: it always succeeds.

use core::fmt;

use tpt_t_core::Mode;
use tpt_t_core::ser::ControlCommand;

use crate::dti::{DomainError, DomainTeleopInterface, OperatorId, Result};
use crate::lifecycle::HandoverState;
use crate::wire::{DomainState, SensorFeed};

/// Why a domain is asking for a human operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum AssistanceReason {
    /// A sensor has failed or is returning implausible data.
    SensorFault = 0,
    /// An actuator or drive has failed.
    ActuatorFault = 1,
    /// The unit is outside its operating envelope.
    GeofenceViolation = 2,
    /// Autonomy is still running but with reduced capability.
    Degraded = 3,
    /// Anything else. Describe it in the domain's own logs.
    Other = 255,
}

/// The tpt-teleop side of `request_teleop_assistance()`.
///
/// The core implements this to receive help requests from a domain. Keeping it
/// a trait means the domain can be tested with a mock link and no network.
pub trait TeleopLink {
    /// Asks for an operator to take over. Returns an error if the request
    /// could not be delivered, in which case the session stays autonomous.
    fn request_teleop_assistance(&mut self, reason: AssistanceReason) -> Result<()>;
}

/// Drives one domain adapter through the handover lifecycle.
pub struct DomainSession<D> {
    adapter: D,
    state: HandoverState,
    operator: Option<OperatorId>,
}

impl<D: DomainTeleopInterface> DomainSession<D> {
    /// Wraps `adapter` in the autonomous state.
    pub fn new(adapter: D) -> Self {
        Self {
            adapter,
            state: HandoverState::Autonomous,
            operator: None,
        }
    }

    /// Current handover state.
    pub fn state(&self) -> HandoverState {
        self.state
    }

    /// The operator currently holding control, if any.
    pub fn operator(&self) -> Option<OperatorId> {
        self.operator
    }

    /// Read-only access to the wrapped adapter.
    pub fn adapter(&self) -> &D {
        &self.adapter
    }

    /// Core-side mode corresponding to the current state.
    pub fn mode(&self) -> Mode {
        self.state.to_mode()
    }

    /// Domain asks for help: `AUTONOMOUS → REQUESTING_TELEOP`.
    ///
    /// The link is called before the state changes, so if delivery fails the
    /// session stays autonomous.
    pub fn request_assistance<L: TeleopLink>(
        &mut self,
        link: &mut L,
        reason: AssistanceReason,
    ) -> Result<()> {
        self.check(
            &[HandoverState::Autonomous],
            HandoverState::RequestingTeleop,
        )?;
        link.request_teleop_assistance(reason)?;
        self.state = HandoverState::RequestingTeleop;
        Ok(())
    }

    /// Withdraws a pending request: `REQUESTING_TELEOP → AUTONOMOUS`.
    pub fn cancel_request(&mut self) -> Result<()> {
        self.check(
            &[HandoverState::RequestingTeleop],
            HandoverState::Autonomous,
        )?;
        self.state = HandoverState::Autonomous;
        Ok(())
    }

    /// An operator takes control: `REQUESTING_TELEOP | RETURNING_TO_AUTONOMY → TELEOP_ACTIVE`.
    pub fn engage(&mut self, operator: OperatorId) -> Result<()> {
        self.check(
            &[
                HandoverState::RequestingTeleop,
                HandoverState::ReturningToAutonomy,
            ],
            HandoverState::TeleopActive,
        )?;
        self.adapter.on_teleop_engage(operator)?;
        self.state = HandoverState::TeleopActive;
        self.operator = Some(operator);
        Ok(())
    }

    /// The operator releases control: `TELEOP_ACTIVE → RETURNING_TO_AUTONOMY`.
    pub fn disengage(&mut self) -> Result<()> {
        self.check(
            &[HandoverState::TeleopActive],
            HandoverState::ReturningToAutonomy,
        )?;
        self.adapter.on_teleop_disengage()?;
        self.state = HandoverState::ReturningToAutonomy;
        self.operator = None;
        Ok(())
    }

    /// The domain has finished ramping back: `RETURNING_TO_AUTONOMY → AUTONOMOUS`.
    pub fn complete_handover(&mut self) -> Result<()> {
        self.check(
            &[HandoverState::ReturningToAutonomy],
            HandoverState::Autonomous,
        )?;
        self.state = HandoverState::Autonomous;
        Ok(())
    }

    /// Forwards one operator command to the adapter. Only valid in
    /// `TELEOP_ACTIVE`.
    ///
    /// A command whose mode is [`Mode::EmergencyStop`] is routed to
    /// [`emergency_stop`](Self::emergency_stop) and not forwarded, so an E-stop
    /// in the command stream always takes effect.
    pub fn command(&mut self, cmd: ControlCommand) -> Result<()> {
        if self.state != HandoverState::TeleopActive {
            return Err(DomainError::NotEngaged { state: self.state });
        }
        if cmd.mode() == Some(Mode::EmergencyStop) {
            self.emergency_stop();
            return Ok(());
        }
        self.adapter.on_control_command(cmd)
    }

    /// Latched hard stop, from any state. Idempotent, and never refused.
    /// The adapter's [`on_emergency_stop`](DomainTeleopInterface::on_emergency_stop)
    /// runs once, on the first transition into `EMERGENCY_STOP`.
    pub fn emergency_stop(&mut self) {
        if self.state == HandoverState::EmergencyStop {
            return;
        }
        self.adapter.on_emergency_stop();
        self.state = HandoverState::EmergencyStop;
        self.operator = None;
    }

    /// Operator acknowledges an emergency stop: `EMERGENCY_STOP → RETURNING_TO_AUTONOMY`.
    /// Control then has to be re-engaged before the unit moves under teleop.
    pub fn acknowledge_emergency_stop(&mut self) -> Result<()> {
        self.check(
            &[HandoverState::EmergencyStop],
            HandoverState::ReturningToAutonomy,
        )?;
        self.state = HandoverState::ReturningToAutonomy;
        Ok(())
    }

    /// Domain state for the operator HUD. Available in every state.
    pub fn domain_state(&self) -> DomainState {
        self.adapter.get_domain_state()
    }

    /// Sensor feed for the operator. Available in every state.
    pub fn sensor_feed(&mut self) -> SensorFeed {
        self.adapter.get_sensor_feed()
    }

    /// Passes only if the session is in one of `from` and `from → to` is legal.
    fn check(&self, from: &[HandoverState], to: HandoverState) -> Result<()> {
        if from.contains(&self.state) && self.state.can_transition_to(to) {
            Ok(())
        } else {
            Err(DomainError::IllegalTransition {
                from: self.state,
                to,
            })
        }
    }
}

impl<D> fmt::Debug for DomainSession<D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DomainSession")
            .field("state", &self.state)
            .field("operator", &self.operator)
            .finish_non_exhaustive()
    }
}
