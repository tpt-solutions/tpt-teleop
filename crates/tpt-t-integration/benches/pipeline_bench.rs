//! Cross-subsystem latency / jitter / throughput benchmark for the full
//! pipeline (Phase 14, item 5). Hand-rolled (no criterion) to keep the
//! dependency tree at zero, matching the other benches in the workspace.
//!
//! Run with: `cargo bench -p tpt-t-integration`

use std::hint::black_box;
use std::time::Instant;

use tpt_t_core::Mode;
use tpt_t_core::ser::ControlCommand;
use tpt_t_integration::PipelineHarness;
use tpt_t_safety::axis;

fn sample(seq: u64, now: u64) -> ControlCommand {
    let mut c = ControlCommand::zeroed(Mode::FullTeleop);
    c.seq = seq;
    c.timestamp_ns = now;
    c.axes[axis::ROLL] = 0.2;
    c.axes[axis::THROTTLE] = 0.55;
    c
}

fn pct(samples: &[u64], p: f64) -> u64 {
    let idx = ((samples.len() - 1) as f64 * p).round() as usize;
    samples[idx]
}

fn main() {
    const ITER: u64 = 200_000;
    let mut h = PipelineHarness::build().unwrap();

    // Warm-up (excluded from all statistics).
    for i in 0..2_000u64 {
        let now = i * 1_000_000;
        let _ = h.step_direct(sample(i, now), now);
    }

    let mut latencies: Vec<u64> = Vec::with_capacity(ITER as usize);
    let t0 = Instant::now();
    for i in 0..ITER {
        let now = (2_000 + i) * 1_000_000;
        let start = Instant::now();
        let sent = black_box(h.pump_forward_direct(sample(2_000 + i, now), now));
        let dt = start.elapsed().as_nanos() as u64;
        let _ = sent;
        latencies.push(dt);
    }
    let elapsed = t0.elapsed();
    latencies.sort_unstable();

    let throughput = ITER as f64 / elapsed.as_secs_f64();
    println!(
        "pipeline end-to-end: n={}  p50={}ns  p90={}ns  p99={}ns  p99.9={}ns  max={}ns",
        ITER,
        pct(&latencies, 0.50),
        pct(&latencies, 0.90),
        pct(&latencies, 0.99),
        pct(&latencies, 0.999),
        latencies[latencies.len() - 1],
    );
    println!(
        "jitter (p99.9 - p50) = {}ns   throughput = {:.0} commands/s",
        pct(&latencies, 0.999) - pct(&latencies, 0.50),
        throughput
    );

    // The <10 µs budget is the *safety intercept* (covered by tpt-t-safety's
    // own bench). This forward data plane (Ingest→Normalize→Route→Safety→
    // Serialize→Transmit) includes the OS UDP send, so report it as
    // informational; the intercept sub-budget is validated separately.
    let p999 = pct(&latencies, 0.999);
    println!(
        "note: safety-intercept sub-budget (<10 µs) is benchmarked in tpt-t-safety; forward data-plane p99.9 = {}ns",
        p999
    );
}
