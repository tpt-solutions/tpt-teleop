//! Telemetry burn-in throughput/latency benchmark for the media layer
//! (Phase 14, item 5). Measures `burn_in_telemetry` — the zero-allocation HUD
//! rasterization of a `TelemetryPacket` into an RGB888 frame — which is the
//! `Media` hot path's deterministic per-frame cost.

use std::hint::black_box;
use std::time::Instant;

use tpt_t_core::ser::TelemetryKind;
use tpt_t_core::ser::TelemetryPacket;
use tpt_t_media::burnin::burn_in_telemetry;
use tpt_t_media::pool::{FrameMeta, PixFmt};

fn pct(s: &[u64], p: f64) -> u64 {
    let i = ((s.len() - 1) as f64 * p).round() as usize;
    s[i]
}

fn main() {
    const ITER: u64 = 50_000;
    let meta = FrameMeta::new(0, 0, 640, 480, PixFmt::Rgb888);
    let mut frame = vec![0u8; PixFmt::Rgb888.min_buffer_len(640, 480)];
    let mut pkt = TelemetryPacket::zeroed(TelemetryKind::Pose, 0, 0);
    pkt.values[0] = 12.5;
    pkt.values[1] = 3.2;

    let mut lat = Vec::with_capacity(ITER as usize);
    let t0 = Instant::now();
    for _ in 0..ITER {
        let start = Instant::now();
        burn_in_telemetry(black_box(&mut frame), &meta, black_box(&pkt), 255);
        lat.push(start.elapsed().as_nanos() as u64);
    }
    let el = t0.elapsed();
    lat.sort_unstable();

    let tp = ITER as f64 / el.as_secs_f64();
    println!(
        "media burn-in: n={} p50={}ns p99={}ns p99.9={}ns max={}ns throughput={:.0} frames/s",
        ITER,
        pct(&lat, 0.50),
        pct(&lat, 0.99),
        pct(&lat, 0.999),
        lat[lat.len() - 1],
        tp
    );
}
