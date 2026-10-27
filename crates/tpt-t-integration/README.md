# tpt-t-integration

End-to-end pipeline tests, zero-allocation and zero-lock verification, and the
cross-subsystem benchmark suite for [tpt-teleop](../../README.md).

```text
Ingest ──▶ Normalize ──▶ Route ──▶ Safety ──▶ Serialize ──▶ Transmit
```

`PipelineHarness` wires these stages in the order the spec prescribes. The
crate proves three things:

- The full data plane works end to end, including a physics-driven `QuadDrone`
  sink that flies the sanitized command inside its envelope.
- The hot path makes no heap allocations (`CountingAllocator`).
- The hot path takes no locks (`tools/lock-audit.sh` and `.ps1`).

## Examples

```bash
cargo run -p tpt-t-integration --example pipeline_demo
cargo run -p tpt-t-integration --example sim_drone
cargo run -p tpt-t-integration --example safety_clamp
```

## Usage

This crate is part of the tpt-teleop workspace and is not published to
crates.io. Run the suite with `cargo test -p tpt-t-integration`, and the
benchmarks with `cargo bench -p tpt-t-integration`.

## Changelog

See [CHANGELOG.md](CHANGELOG.md).

## License

Dual-licensed under MIT OR Apache-2.0, at your option. See the workspace
`LICENSE-MIT` and `LICENSE-APACHE`.
