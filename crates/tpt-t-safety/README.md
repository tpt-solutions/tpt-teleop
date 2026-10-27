# tpt-t-safety

The deterministic real-time safety loop for [tpt-teleop](../../README.md).

- **Geofencing** — keeps commands inside the permitted operating volume.
- **Kinematic limits** — predictive collision avoidance and limit enforcement.
- **Handover smoothing** — spline-smoothed transitions between autonomy and teleop.
- **Latency compensation.**
- **Emergency override** — unconditional, on a dedicated RT thread.

Hot-path contract: pop from the input ring, mutate in place through the
pipeline, push to the output ring. No allocations and no locks.

## Usage

This crate is part of the tpt-teleop workspace and is not published to
crates.io. Depend on it by path or git:

```toml
[dependencies]
tpt-t-safety = { path = "crates/tpt-t-safety" }
```

Run the intercept benchmark with `cargo bench -p tpt-t-safety`.

## Changelog

See [CHANGELOG.md](CHANGELOG.md).

## License

Dual-licensed under MIT OR Apache-2.0, at your option. See the workspace
`LICENSE-MIT` and `LICENSE-APACHE`.
