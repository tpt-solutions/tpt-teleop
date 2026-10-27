//! End-to-end checks of the handover lifecycle through a mock adapter.

use tpt_t_core::Mode;
use tpt_t_core::ser::ControlCommand;
use tpt_t_domain_bridge::{
    AssistanceReason, DomainError, DomainSession, DomainState, DomainTeleopInterface,
    HandoverState, OperatorId, Pose6DOF, Result, SensorFeed, TeleopLink,
    wire::{FAULT_ACTUATOR, HapticFeedback},
};

/// Records every call the session forwards, and can be told to fail.
#[derive(Default)]
struct MockAdapter {
    calls: Vec<String>,
    fail_engage: bool,
    fail_disengage: bool,
}

impl DomainTeleopInterface for MockAdapter {
    fn on_teleop_engage(&mut self, operator: OperatorId) -> Result<()> {
        self.calls
            .push(format!("engage:{}", operator.as_bytes()[0]));
        if self.fail_engage {
            return Err(DomainError::adapter("actuators not ready"));
        }
        Ok(())
    }

    fn on_teleop_disengage(&mut self) -> Result<()> {
        self.calls.push("disengage".into());
        if self.fail_disengage {
            return Err(DomainError::adapter("ramp-down failed"));
        }
        Ok(())
    }

    fn on_control_command(&mut self, cmd: ControlCommand) -> Result<()> {
        self.calls.push(format!("cmd:{}", cmd.seq));
        Ok(())
    }

    fn get_domain_state(&self) -> DomainState {
        DomainState {
            timestamp_us: 0,
            pose: Pose6DOF::IDENTITY,
            battery_pct: 50.0,
            fault_flags: 0,
        }
    }

    fn get_sensor_feed(&mut self) -> SensorFeed {
        SensorFeed {
            timestamp_us: 0,
            video_frames: Vec::new(),
            point_clouds: Vec::new(),
            telemetry: self.get_domain_state(),
            haptic_data: HapticFeedback {
                force_torque: [0.0; 6],
            },
            audio: None,
        }
    }

    fn on_emergency_stop(&mut self) {
        self.calls.push("estop".into());
    }
}

/// Link that either accepts or rejects the help request.
struct MockLink {
    accept: bool,
    requests: Vec<AssistanceReason>,
}

impl MockLink {
    fn accepting() -> Self {
        Self {
            accept: true,
            requests: Vec::new(),
        }
    }
}

impl TeleopLink for MockLink {
    fn request_teleop_assistance(&mut self, reason: AssistanceReason) -> Result<()> {
        self.requests.push(reason);
        if self.accept {
            Ok(())
        } else {
            Err(DomainError::adapter("link down"))
        }
    }
}

fn operator() -> OperatorId {
    OperatorId::new([7; 16])
}

fn command(seq: u64) -> ControlCommand {
    let mut cmd = ControlCommand::zeroed(Mode::FullTeleop);
    cmd.seq = seq;
    cmd
}

/// Brings a fresh session to TELEOP_ACTIVE.
fn engaged() -> DomainSession<MockAdapter> {
    let mut session = DomainSession::new(MockAdapter::default());
    let mut link = MockLink::accepting();
    session
        .request_assistance(&mut link, AssistanceReason::SensorFault)
        .unwrap();
    session.engage(operator()).unwrap();
    session
}

#[test]
fn full_handover_cycle_reaches_the_adapter_in_order() {
    let mut session = DomainSession::new(MockAdapter::default());
    let mut link = MockLink::accepting();

    session
        .request_assistance(&mut link, AssistanceReason::ActuatorFault)
        .unwrap();
    assert_eq!(session.state(), HandoverState::RequestingTeleop);
    assert_eq!(link.requests, vec![AssistanceReason::ActuatorFault]);

    session.engage(operator()).unwrap();
    assert_eq!(session.operator(), Some(operator()));
    session.command(command(1)).unwrap();
    session.command(command(2)).unwrap();
    session.disengage().unwrap();
    assert_eq!(session.operator(), None);
    session.complete_handover().unwrap();

    assert_eq!(session.state(), HandoverState::Autonomous);
    assert_eq!(
        session.adapter().calls,
        ["engage:7", "cmd:1", "cmd:2", "disengage"]
    );
}

#[test]
fn engage_without_a_request_is_refused_and_never_reaches_the_adapter() {
    let mut session = DomainSession::new(MockAdapter::default());
    let err = session.engage(operator()).unwrap_err();
    assert_eq!(
        err,
        DomainError::IllegalTransition {
            from: HandoverState::Autonomous,
            to: HandoverState::TeleopActive,
        }
    );
    assert!(session.adapter().calls.is_empty());
}

#[test]
fn commands_outside_teleop_are_rejected() {
    let mut session = DomainSession::new(MockAdapter::default());
    assert_eq!(
        session.command(command(1)),
        Err(DomainError::NotEngaged {
            state: HandoverState::Autonomous
        })
    );
    assert!(session.adapter().calls.is_empty());
}

#[test]
fn failed_link_keeps_the_session_autonomous() {
    let mut session = DomainSession::new(MockAdapter::default());
    let mut link = MockLink {
        accept: false,
        requests: Vec::new(),
    };
    assert!(
        session
            .request_assistance(&mut link, AssistanceReason::Degraded)
            .is_err()
    );
    assert_eq!(session.state(), HandoverState::Autonomous);
}

#[test]
fn failed_engage_keeps_the_session_requesting() {
    let adapter = MockAdapter {
        fail_engage: true,
        ..MockAdapter::default()
    };
    let mut session = DomainSession::new(adapter);
    let mut link = MockLink::accepting();
    session
        .request_assistance(&mut link, AssistanceReason::SensorFault)
        .unwrap();

    assert!(session.engage(operator()).is_err());
    assert_eq!(session.state(), HandoverState::RequestingTeleop);
    assert_eq!(session.operator(), None);
}

#[test]
fn failed_disengage_keeps_the_operator_in_control() {
    let adapter = MockAdapter {
        fail_disengage: true,
        ..MockAdapter::default()
    };
    let mut session = DomainSession::new(adapter);
    let mut link = MockLink::accepting();
    session
        .request_assistance(&mut link, AssistanceReason::SensorFault)
        .unwrap();
    session.engage(operator()).unwrap();

    assert!(session.disengage().is_err());
    assert_eq!(session.state(), HandoverState::TeleopActive);
    assert_eq!(session.operator(), Some(operator()));
}

#[test]
fn cancel_only_applies_to_a_pending_request() {
    let mut session = DomainSession::new(MockAdapter::default());
    assert!(session.cancel_request().is_err());

    let mut link = MockLink::accepting();
    session
        .request_assistance(&mut link, AssistanceReason::Other)
        .unwrap();
    session.cancel_request().unwrap();
    assert_eq!(session.state(), HandoverState::Autonomous);
}

#[test]
fn disengage_is_refused_during_emergency_stop() {
    let mut session = engaged();
    session.emergency_stop();
    assert!(session.disengage().is_err());
    assert_eq!(session.state(), HandoverState::EmergencyStop);
    assert!(!session.adapter().calls.iter().any(|c| c == "disengage"));
}

#[test]
fn complete_handover_is_refused_while_only_requesting() {
    let mut session = DomainSession::new(MockAdapter::default());
    let mut link = MockLink::accepting();
    session
        .request_assistance(&mut link, AssistanceReason::Other)
        .unwrap();
    assert!(session.complete_handover().is_err());
    assert_eq!(session.state(), HandoverState::RequestingTeleop);
}

#[test]
fn emergency_stop_from_teleop_is_immediate_and_idempotent() {
    let mut session = engaged();
    session.emergency_stop();
    session.emergency_stop();

    assert_eq!(session.state(), HandoverState::EmergencyStop);
    assert_eq!(session.operator(), None);
    assert_eq!(session.mode(), Mode::EmergencyStop);
    let estops = session
        .adapter()
        .calls
        .iter()
        .filter(|c| *c == "estop")
        .count();
    assert_eq!(estops, 1, "adapter must see the E-stop exactly once");
}

#[test]
fn estop_command_in_the_stream_takes_effect_without_forwarding() {
    let mut session = engaged();
    let mut stop = command(9);
    stop.set_mode(Mode::EmergencyStop);

    session.command(stop).unwrap();
    assert_eq!(session.state(), HandoverState::EmergencyStop);
    assert!(!session.adapter().calls.iter().any(|c| c == "cmd:9"));
}

#[test]
fn emergency_stop_exits_only_through_acknowledgment_and_reengage() {
    let mut session = engaged();
    session.emergency_stop();

    // Commands and direct engage are refused until the operator acknowledges.
    assert!(session.command(command(1)).is_err());
    assert!(session.engage(operator()).is_err());

    session.acknowledge_emergency_stop().unwrap();
    assert_eq!(session.state(), HandoverState::ReturningToAutonomy);
    assert_eq!(session.mode(), Mode::Assist);

    session.engage(operator()).unwrap();
    assert_eq!(session.state(), HandoverState::TeleopActive);
}

#[test]
fn acknowledge_is_refused_unless_latched_in_emergency_stop() {
    let mut session = engaged();
    assert!(session.acknowledge_emergency_stop().is_err());
    assert_eq!(session.state(), HandoverState::TeleopActive);
}

#[test]
fn state_and_sensor_queries_work_in_every_state() {
    let mut session = DomainSession::new(MockAdapter::default());
    assert_eq!(session.domain_state().battery_pct, 50.0);
    assert_eq!(session.sensor_feed().telemetry.fault_flags, 0);

    session.emergency_stop();
    assert_eq!(session.domain_state().fault_flags, 0);
    assert_eq!(session.sensor_feed().telemetry.battery_pct, 50.0);
}

#[test]
fn fault_flags_from_the_adapter_reach_the_operator_view() {
    struct Faulty;
    impl DomainTeleopInterface for Faulty {
        fn on_teleop_engage(&mut self, _: OperatorId) -> Result<()> {
            Ok(())
        }
        fn on_teleop_disengage(&mut self) -> Result<()> {
            Ok(())
        }
        fn on_control_command(&mut self, _: ControlCommand) -> Result<()> {
            Ok(())
        }
        fn get_domain_state(&self) -> DomainState {
            DomainState {
                timestamp_us: 0,
                pose: Pose6DOF::IDENTITY,
                battery_pct: 3.0,
                fault_flags: FAULT_ACTUATOR,
            }
        }
        fn get_sensor_feed(&mut self) -> SensorFeed {
            unreachable!("not exercised")
        }
    }

    let session = DomainSession::new(Faulty);
    assert!(!session.domain_state().is_healthy());
}
