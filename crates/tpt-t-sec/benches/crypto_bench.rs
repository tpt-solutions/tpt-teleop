//! Crypto seal/open throughput/latency benchmark for the security layer
//! (Phase 14, item 5). Measures one AES-256-GCM seal followed by its open on
//! the peer box — the `Secure` stage cost per control command, on the zero-copy
//! heap-free path (`seal_into` / `open_into`).

use std::hint::black_box;
use std::time::Instant;

use tpt_t_sec::cipher::{CipherSuite, CryptoBox};

fn pct(s: &[u64], p: f64) -> u64 {
    let i = ((s.len() - 1) as f64 * p).round() as usize;
    s[i]
}

fn main() {
    const ITER: u64 = 200_000;
    let mut key = [0u8; 32];
    for (i, b) in key.iter_mut().enumerate() {
        *b = (i as u8).wrapping_mul(7).wrapping_add(3);
    }
    let box_a = CryptoBox::from_kdf(CipherSuite::Aes256Gcm, &key).unwrap();
    let box_b = CryptoBox::from_kdf(CipherSuite::Aes256Gcm, &key).unwrap();
    let pt = b"take manual control now";

    let mut sealed = vec![0u8; 12 + pt.len() + 16];
    let n = box_a.seal_into(pt, b"aad", &mut sealed).unwrap();
    sealed.truncate(n);
    let mut out = vec![0u8; pt.len()];

    let mut lat = Vec::with_capacity(ITER as usize);
    let t0 = Instant::now();
    for _ in 0..ITER {
        let start = Instant::now();
        let n2 = box_a.seal_into(black_box(pt), b"aad", &mut sealed).unwrap();
        let _ = box_b
            .open_into(black_box(&sealed[..n2]), b"aad", &mut out)
            .unwrap();
        lat.push(start.elapsed().as_nanos() as u64);
    }
    let el = t0.elapsed();
    lat.sort_unstable();

    let tp = ITER as f64 / el.as_secs_f64();
    println!(
        "sec crypto seal+open: n={} p50={}ns p99={}ns p99.9={}ns max={}ns throughput={:.0} ops/s",
        ITER,
        pct(&lat, 0.50),
        pct(&lat, 0.99),
        pct(&lat, 0.999),
        lat[lat.len() - 1],
        tp
    );
}
