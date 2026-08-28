# tpt-teleop

**Tele-Presence Teleoperation** — a hyper-optimized, zero-bloat Rust middleware
workspace for teleoperation of physical robots and drones.

Built for microsecond-level determinism: no async runtime, no serde, no
channels-with-mutexes. Lock-free SPSC rings, zero-copy rkyv serialization,
thread-per-core pinning, and raw OS interfaces all the way down.

## Status

✅ **v1.0.0 released.** All feature phases (0–14) are implemented and verified:
`cargo test`/`clippy`/`fmt` are clean across the workspace, the MIT-chain
dependency audit passes, and the full end-to-end pipeline is proven to make no
per-command heap allocations and take no locks on the hot path. Phase 15 wired
the `tpt-t-sec` crypto/RBAC stack into the live `tpt-t-cloud`/`tpt-t-link`
runtime, and Phase 16/17 add the adoption tooling (scaffold tests, `doctor`,
`sim`, live fleet dashboard, examples) described in [`todo.md`](todo.md).

| CI | Lint | Deps |
|----|------|------|
| ![build](https://github.com/tpt-solutions/tpt-teleop/actions/workflows/ci.yml/badge.svg) | ![lint](https://github.com/tpt-solutions/tpt-teleop/actions/workflows/lint.yml/badge.svg) | cargo-deny (see `deny.toml`) |

> **No system dependencies required.** Everything builds from a stock Rust
> toolchain (`rustc`/`cargo`, edition 2024). There are no C system libraries to
> install: hardware backends (V4L2, SocketCAN, DirectShow, NVENC, …) are reached
> through in-house `libc`/Windows FFI and fail loudly with `Unsupported` until
> hardware bring-up, so the workspace compiles and tests cleanly on a fresh
> Linux, macOS, or Windows machine.

## Getting Started

```bash
# 1. Clone
git clone https://github.com/tpt-solutions/tpt-teleop.git
cd tpt-teleop

# 2. Build the whole workspace (no external system libs needed)
cargo build --workspace

# 3. Run the test suite
cargo test --workspace

# 4. Lint + license audit (CI parity)
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
cargo deny check

# 5. Scaffold a new robot crate and build it
cargo run -p tpt-t-cli -- scaffold my_bot
cd my_bot
cargo build
```

The `tpt-t-cli` developer tool also provides:

* `cargo run -p tpt-t-cli -- deny` — emit a `cargo-deny.toml` for the MIT-chain policy.
* `cargo run -p tpt-t-cli -- profile --cores 8` — emit a CPU core-pinning profile.
* `cargo run -p tpt-t-cli -- doctor` — verify the toolchain/environment.
* `cargo run -p tpt-t-cli -- sim --ticks 200` — live simulator readout (Phase-4 drone + safety loop).
* `cargo run -p tpt-t-cli -- replay <fdr.bin>` — offline FDR replay.
* `cargo run -p tpt-t-cli -- console --host 127.0.0.1:8080` — MCP fleet-dispatch console.

See [`docs/quickstart.md`](docs/quickstart.md) for the `#[derive(tpt_t::Robot)]`
macro quick start, [`docs/phase14-validation.md`](docs/phase14-validation.md) for
the verification + benchmark numbers, and [`todo.md`](todo.md) for the live
roadmap.

## Workspace Crates

| Crate | Purpose |
|-------|---------|
| `tpt-t-core` | Central state machine, autonomy handover, lock-free message bus |
| `tpt-t-ring` | Wait-free SPSC ring buffers, zero-copy pointer passing |
| `tpt-t-link` | UDP multiplexer, io_uring networking, WebRTC transport |
| `tpt-t-input` | Raw HID controller polling, OpenXR hand tracking |
| `tpt-t-media` | Zero-copy camera ingestion, slab allocator, HW encode |
| `tpt-t-safety` | Geofencing, kinematic limits, emergency overrides (RT thread) |
| `tpt-t-hal` | Hardware abstraction: SocketCAN, MAVLink, memory-mapped I/O |
| `tpt-t-cloud` | Custom HTTP/1.1 fleet server, WebRTC SFU, session recording |
| `tpt-t-sec` | Zero-trust access control, E2EE via `ring`, RBAC |
| `tpt-t-analytics` | O_DIRECT flight data recorder, AI training export |
| `tpt-t-cli` | Project scaffolding, cargo-deny config, core-pinning profiles, `sim`/`doctor` |
| `tpt-t-integration` | Phase 14: full-pipeline E2E, zero-alloc/zero-lock verification, benchmarks |

## The Zero-Copy Data Path

```
HID report ──Ingest──▶ Normalize ──Route──▶ Safety loop (in-place)
                                                        │
                                               Serialize ──▶ Transmit ──▶ wire
Total allocations (steady state): 0        Total mutex locks: 0
```

The forward data plane is verified end-to-end in `tpt-t-integration`
(`Ingest → Normalize → Route → Safety → Serialize → Transmit`) and proven to
make no per-command heap allocations and take no locks on the hot path; see
[`tools/lock-audit.sh`](tools/lock-audit.sh) and the `zero_alloc` test.

## Developer Experience

`#[derive(tpt_t::Robot)]` turns a plain struct of device fields into a
lock-free, core-pinned robot: one `SpscRing` per `#[camera]`/`#[motor]` field,
a `launch()` that pins each device to its `CoreProfile` role, and
`serialize_*`/`push_*`/`pop_*` zero-copy wrappers. See
[`docs/quickstart.md`](docs/quickstart.md).

## Licensing

Dual-licensed under either of:

 * Apache License, Version 2.0 — [LICENSE-APACHE](LICENSE-APACHE)
 * MIT license — [LICENSE-MIT](LICENSE-MIT)

Copyright © 2026 TPT Solutions.

Dependency policy ("the MIT chain"): dependencies are restricted to MIT,
BSD-2/3-Clause, ISC, Zlib, and MPL-2.0. Dual MIT/Apache crates are resolved
strictly under MIT; strictly Apache-2.0-only crates are banned. Enforced by
[`deny.toml`](deny.toml) in CI.
