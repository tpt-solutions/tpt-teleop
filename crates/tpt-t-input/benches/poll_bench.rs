//! Ingest + safety-intercept throughput microbenchmark (no criterion).
//!
//! Run with: `cargo bench -p tpt-t-input`

use std::hint::black_box;
use std::sync::Arc;
use std::time::Instant;

use tpt_t_core::ser::ControlCommand;
use tpt_t_core::{Mode, StateMachine};
use tpt_t_input::{ControllerMap, ControllerReport};
use tpt_t_ring::SpscRing;
use tpt_t_safety::{SafetyConfig, SafetyLoop};

fn report() -> ControllerReport {
    ControllerReport {
        seq: 1,
        buttons: 0xFFFF,
        axes: [0.9, -0.4, 0.3, 0.8, 0.1, -0.2, 0.0, 0.5],
        timestamp_ns: 7,
    }
}

fn ingest(map: &ControllerMap, input: &SpscRing<ControlCommand>, i: u64) {
    let mut cmd = ControlCommand::zeroed(Mode::FullTeleop);
    map.apply(&report(), &mut cmd);
    cmd.seq = i as u32;
    let _ = input.push(cmd);
}

fn main() {
    const ITERATIONS: u64 = 500_000;
    let map = ControllerMap::default();
    let input = Arc::new(SpscRing::<ControlCommand>::with_capacity(256));
    let output = Arc::new(SpscRing::<ControlCommand>::with_capacity(256));
    let machine = Arc::new(StateMachine::new());
    let mut l = SafetyLoop::new(
        Arc::clone(&input),
        Arc::clone(&output),
        Arc::clone(&machine),
        SafetyConfig::default(),
    );

    // Warm-up
    for i in 0..1000u64 {
        ingest(&map, &input, black_box(i));
        let _ = black_box(l.process_one(i));
        let _ = black_box(output.pop());
    }

    let t0 = Instant::now();
    for i in 0..ITERATIONS {
        ingest(&map, &input, black_box(i));
        let stats = black_box(l.process_one(i));
        black_box(stats);
        let _ = black_box(output.pop());
    }
    let dt = t0.elapsed();
    println!(
        "ingest+safety: {} iters in {:?} ({:.2} M ticks/s)",
        ITERATIONS,
        dt,
        ITERATIONS as f64 / dt.as_secs_f64() / 1e6
    );
}
