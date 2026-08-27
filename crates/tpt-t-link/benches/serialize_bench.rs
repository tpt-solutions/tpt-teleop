//! Serialization throughput/latency benchmark for the link layer
//! (Phase 14, item 5). Measures `UdpMux::write_control_frame` — the rkyv
//! zero-copy control-command serialize into a reused datagram buffer — which
//! is the `Serialize` stage of the hot data plane.

use std::hint::black_box;
use std::time::Instant;

use tpt_t_core::Mode;
use tpt_t_core::ser::ControlCommand;
use tpt_t_link::mux::{MAX_DATAGRAM, UdpMux};

fn pct(s: &[u64], p: f64) -> u64 {
    let i = ((s.len() - 1) as f64 * p).round() as usize;
    s[i]
}

fn main() {
    const ITER: u64 = 200_000;
    let mut mux = UdpMux::bind_loopback().unwrap();
    let mut buf = [0u8; MAX_DATAGRAM];
    let mut cmd = ControlCommand::zeroed(Mode::FullTeleop);
    cmd.seq = 1;
    cmd.timestamp_ns = 1;

    let mut lat = Vec::with_capacity(ITER as usize);
    let t0 = Instant::now();
    for _ in 0..ITER {
        let start = Instant::now();
        let _ = black_box(mux.write_control_frame(black_box(&cmd), 0, &mut buf));
        lat.push(start.elapsed().as_nanos() as u64);
    }
    let el = t0.elapsed();
    lat.sort_unstable();

    let tp = ITER as f64 / el.as_secs_f64();
    println!(
        "link serialize: n={} p50={}ns p99={}ns p99.9={}ns max={}ns throughput={:.0} cmd/s",
        ITER,
        pct(&lat, 0.50),
        pct(&lat, 0.99),
        pct(&lat, 0.999),
        lat[lat.len() - 1],
        tp
    );
}
