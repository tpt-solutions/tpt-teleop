//! Example 2 — physics-backed drone simulation.
//!
//! Enables the Phase-4 `QuadDrone` sim sink and drives it from synthetic controller
//! reports so you can watch a vehicle fly inside its safety envelope, all through
//! the real safety loop.

use tpt_t_input::ControllerReport;
use tpt_t_integration::PipelineHarness;

fn main() {
    let mut h = PipelineHarness::build().expect("harness");
    h.enable_sim();

    const TICKS: u64 = 120;
    const DT_NS: u64 = 5_000_000;
    for tick in 0..TICKS {
        let now = tick * DT_NS;
        // Gentle, time-varying stick input.
        let roll = (tick as f32 * 0.05).sin() * 0.2;
        let throttle = 0.5 + (tick as f32 * 0.02).cos() * 0.05;
        h.feed_report(ControllerReport {
            seq: tick as u32,
            buttons: 0,
            axes: [roll, 0.0, 0.0, throttle, 0.0, 0.0, 0.0, 0.0],
            timestamp_ns: now,
        });
        let _ = h.step(now);
    }

    let p = h.pose();
    println!("simulated flight complete ({TICKS} ticks):");
    println!("  position x = {:.2} m", p.x);
    println!("  position y = {:.2} m", p.y);
    println!("  altitude   = {:.2} m", p.z);
    println!("  routed     = {}", h.routed());
}
