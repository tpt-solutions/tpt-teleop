# tpt-teleop

**Tele-Presence Teleoperation** — a hyper-optimized, zero-bloat Rust middleware
workspace for teleoperation of physical robots and drones.

Built for microsecond-level determinism: no async runtime, no serde, no
channels-with-mutexes. Lock-free SPSC rings, zero-copy rkyv serialization,
thread-per-core pinning, and raw OS interfaces all the way down.

## Status

🔧 **Phase 14 — Integration, Benchmarking & Release** in progress. All
feature phases (0–13) are implemented, `cargo test`/`clippy`/`fmt` are clean
across the workspace, and the MIT-chain dependency audit passes. Phase 14 adds
the full end-to-end pipeline test, allocation/lock verification tooling, the
cross-subsystem benchmark suite, and the final release audit. See
[`todo.md`](todo.md) for the live checklist and
[`docs/quickstart.md`](docs/quickstart.md) for the macro-driven quick start.

| CI | Lint | Deps |
|----|------|------|
| ![build](https://github.com/tpt-solutions/tpt-teleop/actions/workflows/ci.yml/badge.svg) | ![lint](https://github.com/tpt-solutions/tpt-teleop/actions/workflows/lint.yml/badge.svg) | ![deny](https://github.com/tpt-solutions/tpt-teleop/actions/workflows/deny.yml/badge.svg) |

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
| `tpt-t-cloud` | Custom HTTP/3 fleet server, WebRTC SFU, session recording |
| `tpt-t-sec` | Zero-trust access control, E2EE via `ring`, RBAC |
| `tpt-t-analytics` | O_DIRECT flight data recorder, AI training export |
| `tpt-t-cli` | Project scaffolding, cargo-deny config, core-pinning profiles |
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
