# tpt-teleop

**Tele-Presence Teleoperation** — a hyper-optimized, zero-bloat Rust middleware
workspace for teleoperation of physical robots and drones.

Built for microsecond-level determinism: no async runtime, no serde, no
channels-with-mutexes. Lock-free SPSC rings, zero-copy rkyv serialization,
thread-per-core pinning, and raw OS interfaces all the way down.

## Status

🚧 **Under active development** — see [`todo.md`](todo.md) for the full roadmap
and [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) for the crate map and
zero-copy data-flow design.

| CI | Lint | Deps |
|----|------|------|
| ![build](https://github.com/tpt-solutions/tpt-teleop/actions/workflows/ci.yml/badge.svg) | ![lint](https://github.com/tpt-solutions/tpt-teleop/actions/workflows/lint.yml/badge.svg) | ![deny](https://github.com/tpt-solutions/tpt-teleop/actions/workflows/deny.yml/badge.svg) |

## Workspace Crates

| Crate | Purpose |
|-------|---------|
| `tpt-teleop-core` | Central state machine, autonomy handover, lock-free message bus |
| `tpt-teleop-ring` | Wait-free SPSC ring buffers, zero-copy pointer passing |
| `tpt-teleop-link` | UDP multiplexer, io_uring networking, WebRTC transport |
| `tpt-teleop-input` | Raw HID controller polling, OpenXR hand tracking |
| `tpt-teleop-media` | Zero-copy camera ingestion, slab allocator, HW encode |
| `tpt-teleop-safety` | Geofencing, kinematic limits, emergency overrides (RT thread) |
| `tpt-teleop-hal` | Hardware abstraction: SocketCAN, MAVLink, memory-mapped I/O |
| `tpt-teleop-cloud` | Custom HTTP/3 fleet server, WebRTC SFU, session recording |
| `tpt-teleop-sec` | Zero-trust access control, E2EE via `ring`, RBAC |
| `tpt-teleop-analytics` | O_DIRECT flight data recorder, AI training export |
| `tpt-teleop-cli` | Project scaffolding, cargo-deny config, core-pinning profiles |

## The Zero-Copy Data Path

```
HID report ──cast──▶ ControlCommand ──▶ [ring] ──▶ safety loop (in-place)
                                                        │
                                              [ring] ──▶ rkyv serialize ──▶ UDP
Total allocations: 0        Total mutex locks: 0
```

See `spec.txt` §6 for the authoritative description.

## Licensing

Dual-licensed under either of:

 * Apache License, Version 2.0 — [LICENSE-APACHE](LICENSE-APACHE)
 * MIT license — [LICENSE-MIT](LICENSE-MIT)

Copyright © 2026 TPT Solutions.

Dependency policy ("the MIT chain"): dependencies are restricted to MIT,
BSD-2/3-Clause, ISC, Zlib, and MPL-2.0. Dual MIT/Apache crates are resolved
strictly under MIT; strictly Apache-2.0-only crates are banned. Enforced by
[`deny.toml`](deny.toml) in CI.
