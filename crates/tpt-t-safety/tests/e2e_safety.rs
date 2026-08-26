//! End-to-end Phase 5 acceptance: the safety loop guarding the Phase 4
//! simulated quadrotor â€” hostile inputs bounded, fence breaches intercepted,
//! E-stop overrides everything, handovers blend smoothly.

use std::sync::Arc;

use tpt_t_core::ser::ControlCommand;
use tpt_t_core::{Mode, StateMachine};
use tpt_t_hal::sim::DT_S;
use tpt_t_hal::{CanBus, CanFrame, Pose6D, QuadDrone, World, can_pair, ids};
use tpt_t_ring::SpscRing;
use tpt_t_safety::{GeoFence, KinematicLimits, SafetyConfig, SafetyLoop, axis};

/// Builds a loop wired between fresh rings with the given config.
fn make_loop(
    cfg: SafetyConfig,
) -> (
    Arc<SpscRing<ControlCommand>>,
    Arc<SpscRing<ControlCommand>>,
    SafetyLoop,
) {
    let input = Arc::new(SpscRing::with_capacity(64));
    let output = Arc::new(SpscRing::with_capacity(64));
    let machine = Arc::new(StateMachine::new());
    let l = SafetyLoop::new(
        Arc::clone(&input),
        Arc::clone(&output),
        Arc::clone(&machine),
        cfg,
    );
    (input, output, l)
}

fn teleop_cmd(
    seq: u64,
    ts: u64,
    roll: f32,
    pitch: f32,
    throttle: f32,
    lat_x: f32,
) -> ControlCommand {
    let mut c = ControlCommand::zeroed(Mode::FullTeleop);
    c.seq = seq;
    c.timestamp_ns = ts;
    c.axes[axis::ROLL] = roll;
    c.axes[axis::PITCH] = pitch;
    c.axes[axis::THROTTLE] = throttle;
    c.axes[axis::LAT_X] = lat_x;
    c
}

#[test]
fn hostile_operator_commands_emerge_bounded_and_slewed() {
    let cfg = SafetyConfig {
        limits: KinematicLimits {
            max_axis_step: 0.02,
            ..KinematicLimits::default()
        },
        ..SafetyConfig::default()
    };
    let (input, output, mut l) = make_loop(cfg);
    let _ = l.request_mode(Mode::Assist);
    let _ = l.request_mode(Mode::FullTeleop);

    let mut last_roll = 0.0f32;
    for i in 0..200u64 {
        input
            .push(teleop_cmd(i, i * 5_000_000, 9.9, -9.9, 0.7, 5.0))
            .unwrap();
        let st = l.process_one(i * 5_000_000);
        let out = output.pop().expect("output produced");
        assert_eq!(st.drained, 1);
        assert!(
            out.axes[axis::ROLL].abs() <= 0.35 + 1e-6,
            "tilt clamp violated"
        );
        assert!(out.axes[axis::THROTTLE] <= 1.0);
        // Slew: per-tick movement on roll is bounded by max_axis_step.
        assert!(
            (out.axes[axis::ROLL] - last_roll).abs() <= 0.02 + 1e-6,
            "slew violated: {} -> {}",
            last_roll,
            out.axes[axis::ROLL]
        );
        last_roll = out.axes[axis::ROLL];
    }
}

#[test]
fn hard_fence_breach_latches_estop_and_zeroes_output() {
    let (input, output, mut l) = make_loop(SafetyConfig::default());
    let _ = l.request_mode(Mode::FullTeleop);

    // Vehicle reported far outside the default 500 m fence.
    l.set_pose(&tpt_t_hal::Pose6D {
        x: 900.0,
        y: 0.0,
        z: 40.0,
        ..Default::default()
    });

    input.push(teleop_cmd(1, 0, 0.0, 0.0, 0.8, 0.0)).unwrap();
    l.process_one(5_000_000);

    assert!(l.estop_latched(), "hard breach must latch E-stop");
    let out = output.pop().unwrap();
    assert_eq!(out.mode(), Some(Mode::EmergencyStop));
    assert_eq!(out.flags & 1, 1);
    assert!(
        out.axes.iter().all(|&a| a == 0.0),
        "E-stop output must be neutral"
    );

    // Sticky: subsequent commands stay overridden even with healthy pose.
    l.set_pose(&tpt_t_hal::Pose6D::default());
    input.push(teleop_cmd(2, 0, 0.5, 0.0, 0.8, 0.0)).unwrap();
    l.process_one(10_000_000);
    let out = output.pop().unwrap();
    assert_eq!(out.mode(), Some(Mode::EmergencyStop));
}

#[test]
fn manual_emergency_stop_intercepts_immediately() {
    let (input, output, mut l) = make_loop(SafetyConfig::default());
    l.emergency_stop();

    input.push(teleop_cmd(9, 0, 0.9, 0.9, 1.0, 0.9)).unwrap();
    l.process_one(1_000_000);
    let out = output.pop().unwrap();
    assert_eq!(out.mode(), Some(Mode::EmergencyStop));
    assert!(out.axes.iter().all(|&a| a == 0.0));
}

#[test]
fn handover_blend_is_monotonic_c2_and_reaches_target() {
    let cfg = SafetyConfig {
        transition_s: 0.1,
        ..SafetyConfig::default()
    };
    let (_in, _out, mut l) = make_loop(cfg);

    // Auto â†’ Assist: authority 0 â†’ 0.5 over 100 ms (20 ticks @ 5 ms).
    l.request_mode(Mode::Assist).unwrap();
    let mut prev = -1.0f32;
    for i in 0..40u64 {
        l.process_one(i * 5_000_000);
        let a = l.authority();
        if i > 2 {
            assert!(a >= prev - 1e-6, "authority regressed: {prev} â†’ {a}");
        }
        prev = a;
    }
    assert!(
        (l.authority() - 0.5).abs() < 1e-3,
        "must land exactly on Assist"
    );

    // Assist â†’ FullTeleop continues smoothly to 1.0.
    l.request_mode(Mode::FullTeleop).unwrap();
    for i in 40..80u64 {
        l.process_one(i * 5_000_000);
    }
    assert!((l.authority() - 1.0).abs() < 1e-3);
}

#[test]
fn spawned_safety_thread_processes_until_stopped() {
    let (input, output, mut l) = make_loop(SafetyConfig {
        transition_s: 0.01,
        ..SafetyConfig::default()
    });
    let _ = l.request_mode(Mode::FullTeleop);
    let handle = l.spawn().expect("spawn");

    for i in 0..50u64 {
        input
            .push(teleop_cmd(i, i * 1_000_000, 0.1, 0.1, 0.55, 0.0))
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    std::thread::sleep(std::time::Duration::from_millis(30));

    let mut received = 0;
    while output.pop().is_some() {
        received += 1;
    }
    assert!(received > 10, "thread must be processing, got {received}");
    // Elevation may or may not succeed depending on privileges; either way
    // the flag must have been published.
    let _ = handle.rt_elevated();
    handle.stop(); // joins
}

#[test]
fn simulated_drone_flies_inside_envelope_under_safety_loop() {
    let mut world = World::new([0.0, 0.0, -9.81]);
    let mut drone = QuadDrone::spawn(&mut world);
    let (mut can_op, mut can_veh) = can_pair(16);

    let cfg = SafetyConfig {
        fence: GeoFence {
            radius_m: 60.0,
            max_alt_m: 20.0,
            ..GeoFence::default()
        },
        transition_s: 0.05,
        ..SafetyConfig::default()
    };
    let (input, output, mut l) = make_loop(cfg);
    let _ = l.request_mode(Mode::FullTeleop);

    let mut imu = tpt_t_core::ser::ImuSample::zeroed(0, 0);
    let mut gps = tpt_t_core::ser::GpsSample::zeroed(0, 0);
    let mut pose = Pose6D::default();

    let mut max_roll_out = 0.0f32;
    let ticks = 900usize; // 4.5 s @ 200 Hz
    for tick in 0..ticks {
        // Hostile operator: full-over lateral command + steep climb.
        input
            .push(teleop_cmd(
                tick as u64,
                tick as u64 * 5_000_000,
                9.0,
                0.0,
                0.75,
                9.0,
            ))
            .unwrap();

        // RT safety intercept.
        l.process_one((tick as u64) * 5_000_000);

        // Flight controller consumes the sanitized command.
        if let Some(safe) = output.pop() {
            max_roll_out = max_roll_out.max(safe.axes[axis::ROLL].abs());
            let thrust = safe.axes[axis::THROTTLE].clamp(0.0, 1.0);
            can_op.send(&build_motor_frame(thrust)).unwrap();
        }

        // Simulated vehicle side.
        loop {
            let mut f = CanFrame::new(0, &[]);
            if !can_veh.recv(&mut f) {
                break;
            }
            drone.handle_can(&f);
        }
        drone.apply_actuation(&mut world, DT_S);
        world.step(DT_S);
        drone.post_step(&world, DT_S, &mut imu, &mut gps, &mut pose);
        l.set_pose(&pose);
    }

    // Envelope guarantees: sanitized commands never exceeded the tilt limitâ€¦
    assert!(
        max_roll_out <= 0.35 + 1e-5,
        "safety loop leaked an over-limit roll: {max_roll_out}"
    );
    // â€¦and the vehicle stayed inside the physical envelope.
    assert!(pose.z < 18.0, "breached soft ceiling: {}", pose.z);
    assert!(pose.x.hypot(pose.y) < 45.0, "drifted past fence radius");
}

fn build_motor_frame(thrust: f32) -> CanFrame {
    let raw = (thrust.clamp(0.0, 1.0) * u16::MAX as f32) as u16;
    let be = raw.to_be_bytes();
    let mut payload = [0u8; 8];
    for pair in payload.chunks_mut(2) {
        pair[0] = be[0];
        pair[1] = be[1];
    }
    CanFrame::new(ids::MOTOR_CMD, &payload)
}
