//! <10 µs intercept-latency benchmark for the safety pipeline
//! (spec §5.4 acceptance budget). No criterion — hand-rolled like the ring
//! benches, keeping the dependency tree at zero.
//!
//! Run with: `cargo bench -p tpt-t-safety`

use std::hint::black_box;
use std::sync::Arc;
use std::time::Instant;

use tpt_t_core::ser::ControlCommand;
use tpt_t_core::{Mode, StateMachine};
use tpt_t_ring::SpscRing;
use tpt_t_safety::{SafetyConfig, SafetyLoop};

fn main() {
    const ITERATIONS: u32 = 200_000;
    let input = Arc::new(SpscRing::<ControlCommand>::with_capacity(256));
    let output = Arc::new(SpscRing::<ControlCommand>::with_capacity(256));
    let machine = Arc::new(StateMachine::new());
    // Teleop authority so the blend branch stays hot too.
    let _ = machine.try_transition(Mode::Assist);
    let _ = machine.try_transition(Mode::FullTeleop);

    let mut loop_ = SafetyLoop::new(
        Arc::clone(&input),
        Arc::clone(&output),
        Arc::clone(&machine),
        SafetyConfig {
            transition_s: 0.001,
            ..SafetyConfig::default()
        },
    );
    let pose = tpt_t_hal::Pose6D {
        z: 50.0,
        ..Default::default()
    };
    loop_.set_pose(&pose);

    // Warm-up + fill caches/branch predictors.
    for i in 0..1000u64 {
        input
            .push(ControlCommand {
                seq: i,
                timestamp_ns: i,
                ..ControlCommand::zeroed(Mode::FullTeleop)
            })
            .unwrap();
        let _ = black_box(loop_.process_one(i * 100));
        let _ = black_box(output.pop());
    }

    let mut samples: Vec<u32> = Vec::with_capacity(ITERATIONS as usize);
    for i in 0..ITERATIONS as u64 {
        input
            .push(ControlCommand {
                seq: i,
                timestamp_ns: i,
                ..ControlCommand::zeroed(Mode::FullTeleop)
            })
            .unwrap();
        let t0 = Instant::now();
        let stats = black_box(loop_.process_one(black_box(i * 100)));
        let dt = t0.elapsed().as_nanos() as u32;
        black_box(stats);
        samples.push(dt);
        let _ = black_box(output.pop());
    }

    samples.sort_unstable();
    let pct = |p: f64| -> u32 {
        let idx = ((samples.len() - 1) as f64 * p).round() as usize;
        samples[idx]
    };
    println!(
        "safety intercept: n={} p50={}ns p90={}ns p99={}ns p99.9={}ns max={}ns",
        samples.len(),
        pct(0.50),
        pct(0.90),
        pct(0.99),
        pct(0.999),
        samples[samples.len() - 1],
    );

    let under_budget = pct(0.999) < 10_000;
    println!(
        "p99.9 < 10 µs budget: {}",
        if under_budget { "PASS" } else { "FAIL" }
    );
}
