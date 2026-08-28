//! `tpt-t-cli sim` — live terminal readout driving the Phase 4 simulator + the
//! full safety loop end-to-end (Phase 17).
//!
//! Drives `tpt_t_integration::PipelineHarness` — Ingest → Normalize → Route →
//! Safety → Serialize → Transmit — with a physics-backed `QuadDrone` sink, so a
//! developer can watch a simulated vehicle fly inside its safety envelope without
//! writing any test code. No new external dependencies: it reuses the exact
//! harness the Phase 14 end-to-end tests exercise.
//!
//! ```text
//! tpt-t-cli sim [--ticks <N>] [--rate <HZ>] [--throttle <0..1>] [--roll <RAD>]
//! ```

use std::thread;
use std::time::Duration;

use tpt_t_input::ControllerReport;
use tpt_t_integration::PipelineHarness;
use tpt_t_safety::axis;

/// Control tick period (5 ms), matching the Phase 14 e2e harness.
const DT_NS: u64 = 5_000_000;

/// sim subcommand entry point.
pub fn run(args: &[String]) -> i32 {
    let mut ticks: u64 = 200;
    let mut rate_hz: f64 = 20.0;
    let mut throttle: f32 = 0.55;
    let mut roll: f32 = 0.0;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--ticks" => {
                i += 1;
                if let Some(v) = args.get(i).and_then(|s| s.parse::<u64>().ok()) {
                    ticks = v;
                } else {
                    eprintln!("error: --ticks requires a number");
                    return 1;
                }
            }
            "--rate" => {
                i += 1;
                if let Some(v) = args.get(i).and_then(|s| s.parse::<f64>().ok()) {
                    rate_hz = v.max(1.0);
                } else {
                    eprintln!("error: --rate requires a number");
                    return 1;
                }
            }
            "--throttle" => {
                i += 1;
                if let Some(v) = args.get(i).and_then(|s| s.parse::<f32>().ok()) {
                    throttle = v.clamp(0.0, 1.0);
                } else {
                    eprintln!("error: --throttle requires a number");
                    return 1;
                }
            }
            "--roll" => {
                i += 1;
                if let Some(v) = args.get(i).and_then(|s| s.parse::<f32>().ok()) {
                    roll = v;
                } else {
                    eprintln!("error: --roll requires a number");
                    return 1;
                }
            }
            other => {
                eprintln!("error: unrecognized argument {other:?}");
                return 1;
            }
        }
        i += 1;
    }

    simulate(ticks, rate_hz, throttle, roll)
}

/// Builds the harness, drives `ticks` control ticks, and prints a live readout.
/// Returns the process exit code.
pub fn simulate(ticks: u64, rate_hz: f64, throttle: f32, base_roll: f32) -> i32 {
    let mut h = match PipelineHarness::build() {
        Ok(h) => h,
        Err(e) => {
            eprintln!("error: cannot build pipeline harness: {e}");
            return 1;
        }
    };
    h.enable_sim();

    println!("tpt-teleop simulator — Phase-4 drone + safety loop");
    println!("tick |    x(m)     y(m)     z(m) | roll_cmd  throttle | routed");
    println!("-----+----------------------------------+------------------+-------");

    let frame = Duration::from_secs_f64(1.0 / rate_hz);
    for tick in 0..ticks {
        let now = tick * DT_NS;
        // Oscillate roll around the requested base so the drone actually moves
        // and the safety loop has something to clamp.
        let roll = base_roll + (tick as f32 * 0.05).sin() * 0.2;
        let thr = (throttle + (tick as f32 * 0.02).cos() * 0.05).clamp(0.0, 1.0);
        h.feed_report(ControllerReport {
            seq: tick as u32,
            buttons: 0,
            axes: [roll, 0.0, 0.0, thr, 0.0, 0.0, 0.0, 0.0],
            timestamp_ns: now,
        });

        let rx = match h.step(now) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("error: pipeline step failed: {e}");
                return 1;
            }
        };
        let p = h.pose();
        let (rc, tc) = match rx {
            Some(c) => (c.axes[axis::ROLL], c.axes[axis::THROTTLE]),
            None => (0.0, 0.0),
        };
        println!(
            "{:>4} | {:>8.2} {:>8.2} {:>8.2} | {:>8.3} {:>10.3} | {}",
            tick,
            p.x,
            p.y,
            p.z,
            rc,
            tc,
            h.routed()
        );
        thread::sleep(frame);
    }

    let p = h.pose();
    println!(
        "\nfinal pose: x={:.2} y={:.2} z={:.2}  routed={}",
        p.x,
        p.y,
        p.z,
        h.routed()
    );
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drives_the_pipeline_and_sim_without_panicking() {
        // High rate + tiny tick count keeps the test fast; the point is that the
        // harness, safety loop, and physics sink all advance end to end.
        assert_eq!(simulate(5, 1000.0, 0.5, 0.0), 0);
    }

    #[test]
    fn rejects_bad_arguments() {
        assert_eq!(run(&["--ticks".into(), "notanumber".into()]), 1);
        assert_eq!(run(&["--bogus".into()]), 1);
    }
}
