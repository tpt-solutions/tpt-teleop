//! Full-pipeline end-to-end acceptance (Phase 14, item 1).
//!
//! Proves the complete zero-copy data plane works as one unit:
//! Ingest → Normalize → Route → Safety → Serialize → Transmit, with the
//! transmitted command demultiplexed back and validated on the wire.

use tpt_t_core::Mode;
use tpt_t_core::ser::ControlCommand;
use tpt_t_input::ControllerReport;
use tpt_t_integration::PipelineHarness;
use tpt_t_safety::{GeoFence, KinematicLimits, SafetyConfig, axis};

/// Safety config that applies no kinematic limits, so a within-envelope
/// command flows through the safety loop unchanged.
fn passthrough_cfg() -> SafetyConfig {
    SafetyConfig {
        limits: KinematicLimits {
            max_tilt_norm: 10.0,
            max_yaw_norm: 10.0,
            max_lateral_norm: 10.0,
            max_axis_step: 10.0,
            max_throttle_step: 10.0,
        },
        ..SafetyConfig::default()
    }
}

/// Builds a command with sane timestamps (so latency compensation is a no-op).
fn cmd(seq: u64, now: u64, roll: f32, throttle: f32) -> ControlCommand {
    let mut c = ControlCommand::zeroed(Mode::FullTeleop);
    c.seq = seq;
    c.timestamp_ns = now;
    c.axes[axis::ROLL] = roll;
    c.axes[axis::THROTTLE] = throttle;
    c
}

#[test]
fn command_preserved_end_to_end_over_real_udp() {
    let mut h = PipelineHarness::build_with(passthrough_cfg()).unwrap();
    let now = 1_000_000u64;
    let rx = h
        .step_direct(cmd(42, now, 0.3, 0.6), now)
        .unwrap()
        .expect("command received");

    assert_eq!(rx.seq, 42, "sequence must survive the wire");
    assert_eq!(rx.mode(), Some(Mode::FullTeleop), "mode must survive");
    assert!((rx.axes[axis::ROLL] - 0.3).abs() < 1e-3, "roll altered");
    assert!(
        (rx.axes[axis::THROTTLE] - 0.6).abs() < 1e-3,
        "throttle altered"
    );
    assert!(h.routed() >= 1, "routing stage must have fired");
}

#[test]
fn ai_origin_tag_survives_the_pipeline() {
    let mut h = PipelineHarness::build_with(passthrough_cfg()).unwrap();
    let now = 2_000_000u64;
    let mut c = cmd(7, now, 0.0, 0.5);
    c.set_ai_origin();
    let rx = h.step_direct(c, now).unwrap().expect("command received");

    assert!(
        rx.is_ai_origin(),
        "AI origin flag must survive Route→Transmit"
    );
    assert_eq!(rx.seq, 7);
}

#[test]
fn safety_clamps_and_slews_before_the_command_is_transmitted() {
    // Default config: tilt clamp 0.35, per-tick slew 0.02, throttle [0,1].
    let mut h = PipelineHarness::build_with(SafetyConfig::default()).unwrap();

    let mut prev_roll = 0.0f32;
    let mut peak_roll = 0.0f32;
    for i in 0..200u64 {
        let now = i * 1_000_000;
        let rx = h
            .step_direct(cmd(i, now, 9.9, 9.9), now)
            .unwrap()
            .expect("command received");

        assert!(
            rx.axes[axis::ROLL].abs() <= 0.35 + 1e-4,
            "tilt leak past kinematic clamp: {}",
            rx.axes[axis::ROLL]
        );
        assert!(
            rx.axes[axis::THROTTLE] <= 1.0 + 1e-6,
            "throttle exceeded 1.0"
        );
        let step = (rx.axes[axis::ROLL] - prev_roll).abs();
        assert!(step <= 0.02 + 1e-4, "slew-rate violated: {step}");
        assert_eq!(rx.mode(), Some(Mode::FullTeleop), "no breach at origin");
        peak_roll = peak_roll.max(rx.axes[axis::ROLL].abs());
        prev_roll = rx.axes[axis::ROLL];
    }
    assert!(
        peak_roll > 0.34,
        "should ramp to the tilt clamp ceiling, got {peak_roll}"
    );
}

#[test]
fn ingest_normalize_then_wire_round_trips_axes() {
    let mut h = PipelineHarness::build_with(passthrough_cfg()).unwrap();
    let now = 3_000_000u64;
    // Raw report: roll slot 0.5, throttle slot (3) 0.6.
    h.feed_report(ControllerReport {
        seq: 5,
        buttons: 0,
        axes: [0.5, 0.0, 0.0, 0.6, 0.0, 0.0, 0.0, 0.0],
        timestamp_ns: now,
    });
    let rx = h.step(now).unwrap().expect("command received");

    // Normalize: roll = (0.5 - deadzone 0.04) / (1 - 0.04); throttle = 0.6*0.5+0.5.
    assert!((rx.axes[axis::ROLL] - (0.5 - 0.04) / 0.96).abs() < 1e-3);
    assert!((rx.axes[axis::THROTTLE] - 0.8).abs() < 1e-3);
    assert_eq!(rx.mode(), Some(Mode::FullTeleop));
}

#[test]
fn full_pipeline_flies_simulated_drone_inside_envelope() {
    let mut h = PipelineHarness::build_with(SafetyConfig {
        fence: GeoFence {
            radius_m: 60.0,
            max_alt_m: 20.0,
            ..GeoFence::default()
        },
        transition_s: 0.05,
        ..SafetyConfig::default()
    })
    .unwrap();
    h.enable_sim();

    let mut max_roll = 0.0f32;
    for tick in 0..900u64 {
        let now = tick * 5_000_000;
        // Hostile operator: saturated tilt + lateral + steep climb every tick.
        h.feed_report(ControllerReport {
            seq: tick as u32,
            buttons: 0,
            axes: [9.0, 0.0, 0.0, 0.75, 9.0, 0.0, 0.0, 0.0],
            timestamp_ns: now,
        });
        let rx = h.step(now).unwrap().expect("command received");
        max_roll = max_roll.max(rx.axes[axis::ROLL].abs());
    }

    let p = h.pose();
    // Envelope guarantees hold end-to-end *including* the wire round-trip.
    // These bounds are the configured geofence (radius 60 m, max alt 20 m);
    // the safety loop + geofence clamp must keep the vehicle strictly inside.
    assert!(
        max_roll <= 0.35 + 1e-4,
        "safety loop leaked an over-limit roll through the wire: {max_roll}"
    );
    assert!(p.z < 21.0, "breached ceiling: {}", p.z);
    assert!(
        p.x.hypot(p.y) < 62.0,
        "drifted past fence radius: {}",
        p.x.hypot(p.y)
    );
}
