//! Hand-rolled benchmarks: no criterion, no extra deps (zero-bloat policy).
//!
//! Run with: `cargo bench -p tpt-t-ring`

use std::hint::spin_loop;
use std::sync::Arc;
use std::thread;
use std::time::Instant;

use tpt_t_ring::SpscRing;

fn main() {
    bench_throughput();
    bench_ping_pong_latency();
}

/// Single-threaded push+pop throughput at various capacities.
fn bench_throughput() {
    println!("== SPSC throughput (single thread, push+pop pairs) ==");
    const N: u64 = 20_000_000;
    for cap in [64usize, 1024, 65_536] {
        let ring: SpscRing<u64> = SpscRing::with_capacity(cap);
        let start = Instant::now();
        let mut i = 0u64;
        while i < N {
            // Fill then drain: exercises both full-ring rejection and empty.
            while ring.push(i).is_ok() {
                i += 1;
                if i >= N {
                    break;
                }
            }
            while ring.pop().is_some() {}
        }
        let elapsed = start.elapsed();
        let ops = N as f64 + N as f64; // pushes + pops
        let mops = ops / elapsed.as_secs_f64() / 1e6;
        println!("  cap {:>6}: {:>10.1} Mops/s ({:?})", cap, mops, elapsed);
    }
}

/// Cross-thread ping-pong latency distribution over a 1-slot request ring
/// and a 1-slot response ring — the tightest possible RTT microbenchmark,
/// representative of control-command round trips.
fn bench_ping_pong_latency() {
    println!("== SPSC ping-pong latency (cross-thread RTT) ==");
    const ITERATIONS: usize = 200_000;
    let req: Arc<SpscRing<u64>> = Arc::new(SpscRing::with_capacity(1));
    let resp: Arc<SpscRing<u64>> = Arc::new(SpscRing::with_capacity(1));

    let echo_req = Arc::clone(&req);
    let echo_resp = Arc::clone(&resp);
    let echoer = thread::spawn(move || {
        loop {
            match echo_req.pop() {
                Some(v) => {
                    while echo_resp.push(v.wrapping_add(1)).is_err() {
                        spin_loop();
                    }
                }
                None => spin_loop(),
            }
        }
    });

    // Warm-up
    for i in 0..1_000u64 {
        ping_pong_once(&req, &resp, i);
    }

    let mut samples: Vec<u32> = Vec::with_capacity(ITERATIONS);
    for i in 0..ITERATIONS as u64 {
        let start = Instant::now();
        ping_pong_once(&req, &resp, i);
        samples.push(start.elapsed().as_nanos() as u32);
    }

    drop(req); // not enough to stop echoer; it just spins harmlessly
    samples.sort_unstable();
    let pct = |p: f64| -> u32 {
        let idx = ((samples.len() - 1) as f64 * p).round() as usize;
        samples[idx]
    };
    println!(
        "  n={} p50={}ns p90={}ns p99={}ns p99.9={}ns max={}ns",
        samples.len(),
        pct(0.50),
        pct(0.90),
        pct(0.99),
        pct(0.999),
        samples[samples.len() - 1],
    );

    // Shut the echoer down politely by dropping both rings' senders is not
    // possible without a flag; detach — process exits right after.
    echoer.thread().unpark();
}

fn ping_pong_once(req: &SpscRing<u64>, resp: &SpscRing<u64>, v: u64) -> u64 {
    while req.push(v).is_err() {
        spin_loop();
    }
    loop {
        if let Some(back) = resp.pop() {
            return back;
        }
        spin_loop();
    }
}
