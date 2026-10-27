//! The universal autonomy ↔ teleop handover state machine (spec §4.3).
//!
//! ```text
//! AUTONOMOUS → REQUESTING_TELEOP → TELEOP_ACTIVE → RETURNING_TO_AUTONOMY → AUTONOMOUS
//!                  │ (cancel)  ↑                        │ (re-engage)
//!                  └───────────┘                        └──→ TELEOP_ACTIVE
//! any state → EMERGENCY_STOP → RETURNING_TO_AUTONOMY (operator acknowledgment)
//! ```
//!
//! This is the domain-side view. The core [`StateMachine`](tpt_t_core::StateMachine)
//! keeps the authoritative [`Mode`]; [`HandoverState::to_mode`] gives the
//! corresponding mode for each state.

use tpt_t_core::Mode;

/// Handover state of one domain session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum HandoverState {
    /// The robot runs its own autonomy. No operator holds it.
    Autonomous = 0,
    /// The domain has asked for help and is waiting for an operator.
    RequestingTeleop = 1,
    /// An operator is in control.
    TeleopActive = 2,
    /// The operator released control and the domain is ramping back to autonomy.
    ReturningToAutonomy = 3,
    /// Latched hard stop. Exit only via [`ReturningToAutonomy`](Self::ReturningToAutonomy).
    EmergencyStop = 4,
}

impl HandoverState {
    /// Spec-style name, as used in logs and HUDs.
    pub const fn name(self) -> &'static str {
        match self {
            HandoverState::Autonomous => "AUTONOMOUS",
            HandoverState::RequestingTeleop => "REQUESTING_TELEOP",
            HandoverState::TeleopActive => "TELEOP_ACTIVE",
            HandoverState::ReturningToAutonomy => "RETURNING_TO_AUTONOMY",
            HandoverState::EmergencyStop => "EMERGENCY_STOP",
        }
    }

    /// Whether the lifecycle permits `self → to`.
    pub fn can_transition_to(self, to: HandoverState) -> bool {
        use HandoverState::*;
        // Emergency stop is reachable from every state, including itself
        // (idempotent), so only the other edges need listing.
        if to == EmergencyStop {
            return true;
        }
        matches!(
            (self, to),
            (Autonomous, RequestingTeleop)
                | (RequestingTeleop, Autonomous)
                | (RequestingTeleop, TeleopActive)
                | (TeleopActive, ReturningToAutonomy)
                | (ReturningToAutonomy, TeleopActive)
                | (ReturningToAutonomy, Autonomous)
                | (EmergencyStop, ReturningToAutonomy)
        )
    }

    /// The core [`Mode`] that corresponds to this handover state.
    ///
    /// `RequestingTeleop` is still autonomous, so it maps to `Auto`.
    /// `ReturningToAutonomy` is blended, so it maps to `Assist`.
    pub fn to_mode(self) -> Mode {
        match self {
            HandoverState::Autonomous | HandoverState::RequestingTeleop => Mode::Auto,
            HandoverState::ReturningToAutonomy => Mode::Assist,
            HandoverState::TeleopActive => Mode::FullTeleop,
            HandoverState::EmergencyStop => Mode::EmergencyStop,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [HandoverState; 5] = [
        HandoverState::Autonomous,
        HandoverState::RequestingTeleop,
        HandoverState::TeleopActive,
        HandoverState::ReturningToAutonomy,
        HandoverState::EmergencyStop,
    ];

    #[test]
    fn happy_path_is_allowed() {
        use HandoverState::*;
        assert!(Autonomous.can_transition_to(RequestingTeleop));
        assert!(RequestingTeleop.can_transition_to(TeleopActive));
        assert!(TeleopActive.can_transition_to(ReturningToAutonomy));
        assert!(ReturningToAutonomy.can_transition_to(Autonomous));
    }

    #[test]
    fn engage_without_request_is_refused() {
        assert!(!HandoverState::Autonomous.can_transition_to(HandoverState::TeleopActive));
    }

    #[test]
    fn emergency_stop_is_reachable_from_everywhere() {
        for s in ALL {
            assert!(s.can_transition_to(HandoverState::EmergencyStop), "{s:?}");
        }
    }

    #[test]
    fn emergency_stop_exits_only_to_returning() {
        for to in ALL {
            let allowed = HandoverState::EmergencyStop.can_transition_to(to);
            let expected = matches!(
                to,
                HandoverState::ReturningToAutonomy | HandoverState::EmergencyStop
            );
            assert_eq!(allowed, expected, "EMERGENCY_STOP -> {to:?}");
        }
    }

    #[test]
    fn mode_mapping_covers_every_state() {
        assert_eq!(HandoverState::Autonomous.to_mode(), Mode::Auto);
        assert_eq!(HandoverState::RequestingTeleop.to_mode(), Mode::Auto);
        assert_eq!(HandoverState::ReturningToAutonomy.to_mode(), Mode::Assist);
        assert_eq!(HandoverState::TeleopActive.to_mode(), Mode::FullTeleop);
        assert_eq!(HandoverState::EmergencyStop.to_mode(), Mode::EmergencyStop);
    }
}
