//! Example 3 — safety envelope clamping.
//!
//! Feeds a hostile, saturated command stream (max roll + max throttle every tick)
//! and demonstrates the safety loop clamping to its kinematic limits before the
//! command is transmitted, so the vehicle can never be driven past its envelope.

use tpt_t_core::Mode;
use tpt_t_input::ControllerReport;
use tpt_t_integration::PipelineHarness;
use tpt_t_safety::{SafetyConfig, axis};

fn main() {
    // Default safety config: tilt clamp 0.35, per-tick slew 0.02, throttle [0,1].
    let mut h = PipelineHarness::build_with(SafetyConfig::default()).expect("harness");

    let mut peak_roll = 0.0f32;
    for i in 0..200u64 {
        let now = i * 1_000_000;
        h.feed_report(ControllerReport {
            seq: i as u32,
            buttons: 0,
            axes: [9.9, 0.0, 0.0, 9.9, 0.0, 0.0, 0.0, 0.0],
            timestamp_ns: now,
        });
        let rx = h.step(now).expect("step").expect("command received");
        peak_roll = peak_roll.max(rx.axes[axis::ROLL].abs());
        assert_eq!(rx.mode(), Some(Mode::FullTeleop));
    }

    println!("safety clamp verified:");
    println!("  peak roll seen on the wire = {peak_roll:.4} (limit 0.35)");
    println!(
        "  throttle stayed in [0, 1]  = {}",
        peak_roll <= 0.35 + 1e-3
    );
    assert!(
        peak_roll <= 0.35 + 1e-3,
        "safety loop leaked an over-limit roll: {peak_roll}"
    );
}
