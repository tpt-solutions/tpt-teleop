//! Lock-free message-bus throughput/latency benchmark for the core layer
//! (Phase 14, item 5). Measures the `Route` stage: a single publish fanned out
//! and drained by a subscriber — the fan-out primitive the safety loop and the
//! FDR logger both consume.

use std::hint::black_box;
use std::time::Instant;

use tpt_t_core::Mode;
use tpt_t_core::bus::MessageBus;
use tpt_t_core::ser::ControlCommand;

fn pct(s: &[u64], p: f64) -> u64 {
    let i = ((s.len() - 1) as f64 * p).round() as usize;
    s[i]
}

fn main() {
    const ITER: u64 = 1_000_000;
    let mut bus: MessageBus<ControlCommand> = MessageBus::new(1024);
    let sub = bus.subscribe();
    let mut cmd = ControlCommand::zeroed(Mode::FullTeleop);
    let mut drain = Vec::with_capacity(64);

    let mut lat = Vec::with_capacity(ITER as usize);
    let t0 = Instant::now();
    for i in 0..ITER {
        cmd.seq = i;
        let start = Instant::now();
        bus.publish(black_box(cmd));
        bus.poll(sub, &mut drain);
        lat.push(start.elapsed().as_nanos() as u64);
    }
    let el = t0.elapsed();
    lat.sort_unstable();

    let tp = ITER as f64 / el.as_secs_f64();
    println!(
        "core bus publish+poll: n={} p50={}ns p99={}ns p99.9={}ns max={}ns throughput={:.0} ops/s",
        ITER,
        pct(&lat, 0.50),
        pct(&lat, 0.99),
        pct(&lat, 0.999),
        lat[lat.len() - 1],
        tp
    );
}
