# tpt-t-core

The central runtime for [tpt-teleop](../../README.md): state machine, lock-free
message bus, platform event loops, thread pinning, and zero-copy wire types.

## Modules

- `mode` / `machine` — Auto, Assist, FullTeleop, and EmergencyStop modes, with
  an atomic transition engine for autonomy handover.
- `mpmc` — bounded lock-free MPMC queue (Vyukov-style).
- `bus` — fan-out message bus built on `tpt-t-ring` SPSC queues.
- `pool` — pre-allocated buffer pool with zero steady-state allocation.
- `ser` — rkyv wire types (`ControlCommand`, telemetry) and zero-copy
  serialize/deserialize helpers.
- `eventloop` — io_uring (Linux), kqueue (macOS), and IOCP (Windows).
- `affinity` — thread-per-core pinning.
- `profile` — CPU core-pinning role profiles and their config format.

## Usage

This crate is part of the tpt-teleop workspace and is not published to
crates.io. Depend on it by path or git:

```toml
[dependencies]
tpt-t-core = { path = "crates/tpt-t-core" }
```

Run the benchmarks with `cargo bench -p tpt-t-core`.

## Changelog

See [CHANGELOG.md](CHANGELOG.md).

## License

Dual-licensed under MIT OR Apache-2.0, at your option. See the workspace
`LICENSE-MIT` and `LICENSE-APACHE`.
