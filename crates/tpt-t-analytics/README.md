# tpt-t-analytics

Flight Data Recorder (FDR) and AI-training export for [tpt-teleop](../../README.md).

Logging must never block the control loop. The hot path publishes fixed-size
`FdrEntry` frames through a wait-free SPSC ring; a dedicated storage thread
drains the ring and writes straight to disk with direct I/O (`O_DIRECT` on
Linux, `FILE_FLAG_NO_BUFFERING` on Windows, `F_NOCACHE` on macOS), so the page
cache never stalls the writer. If the ring is full, the record is shed rather
than blocking the producer.

## Features

- Direct-I/O FDR writer over `tpt_t_ring::SpscRing`.
- Offline export of recorded sessions to NumPy `.npy` arrays, readable with
  `numpy.load` for both PyTorch and JAX.

## Usage

This crate is part of the tpt-teleop workspace and is not published to
crates.io. Depend on it by path or git:

```toml
[dependencies]
tpt-t-analytics = { path = "crates/tpt-t-analytics" }
```

See the crate docs (`cargo doc -p tpt-t-analytics --open`) for the API.

## Changelog

See [CHANGELOG.md](CHANGELOG.md).

## License

Dual-licensed under MIT OR Apache-2.0, at your option. See the workspace
`LICENSE-MIT` and `LICENSE-APACHE`.
