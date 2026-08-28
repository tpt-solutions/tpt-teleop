# Changelog

All notable changes to tpt-teleop are documented here. The project follows
semantic versioning; the workspace is released as a single `vX.Y.Z` tag.

## [1.0.0] — 2026-08-27

First production release. The complete zero-copy, lock-free teleoperation
middleware, cross-platform (Linux/macOS/Windows) and free of Apache-2.0-only
dependencies (the "MIT chain" enforced by `deny.toml`).

### Implemented phases (0–14)
- **Foundation & tooling** — workspace, license metadata, `cargo-deny` policy, CI.
- **Lock-free core** — wait-free SPSC ring, zero-copy struct casting, central
  state machine, lock-free message bus.
- **Cross-platform runtime** — `io_uring`/`kqueue`/IOCP event loop, thread-per-core
  pinning with a core-profile config format.
- **Zero-copy serialization** — rkyv `ControlCommand`/telemetry structs and a
  pre-allocated buffer pool.
- **Mock hardware / simulator** — HAL trait, mock CAN/motor/sensor backends, and a
  deterministic in-house rigid-body `QuadDrone` sim (rapier dropped for the MIT
  chain).
- **Safety & autonomy** — RT intercept thread, geofencing, kinematic limits,
  spline handover, latency compensation, emergency override, shared-control
  arbitration.
- **Input** — raw HID polling (Linux/macOS/Windows), OpenXR pose types, haptics,
  multi-operator state, AI input source with origin tagging.
- **Networking** — custom UDP multiplexer over one port, neighbor-discovery mesh,
  backpressure throttling, in-house selective-repeat reliable transport.
- **Media** — slab allocator, zero-copy capture/encode backends (loudly
  `Unsupported` pre-hardware), telemetry burn-in, backpressure-driven bitrate.
- **HAL completion** — raw SocketCAN, from-scratch MAVLink 1.0/2.0 parser, MMIO.
- **Cloud** — in-house HTTP/1.1 fleet server, `SfuFanout`, session recorder,
  multi-unit orchestration, MCP dispatch over JSON-RPC 2.0.
- **Security** — `ring`-backed AES-256-GCM / ChaCha20-Poly1305, zero-trust RBAC,
  integrated into link/cloud.
- **Analytics** — O_DIRECT / `FILE_FLAG_NO_BUFFERING` / `F_NOCACHE` FDR logging
  over a wait-free ring, rkyv → `.npy` export for PyTorch/JAX.
- **Developer experience** — `#[derive(tpt_t::Robot)]` macro, `tpt-t-cli`
  scaffold/deny/profile, macro quick-start.
- **Integration & release** — full-pipeline E2E, zero-alloc/zero-lock verification,
  cross-subsystem benchmarks, tagged `v1.0.0`.

## [1.1.0] — 2026-08-28

Adoption tooling and demoability (Phases 15–17). No breaking API changes.

### Added
- **Phase 15 — Security hardening:** `tpt-t-sec` crypto/RBAC is now live in
  `tpt-t-cloud`/`tpt-t-link`: `FleetAuthz`/`Principal` gate the HTTP fleet API
  and MCP dispatch (401/403 on missing/insufficient attestation);
  `SecureUdpTransport` runs encrypted per-peer sessions bootstrapped via
  `POST /api/units/:id/secure/handshake`; `Channel::Secure` framing is wired
  end-to-end; the AAD mismatch (telemetry now decrypts under its own AAD) is
  fixed; mesh beacons reject implausible timestamps and rate-limit address
  flapping; `FleetServer` gained a connection cap and idle sweep.
- **Phase 16 — Adoption tooling:**
  - `tpt-t-cli scaffold` output now compiles end-to-end (proven by a real
    scaffold-then-build test); generated `Camera`/`Motor` devices actually loop
    (push/pop real data) and `main` joins the device handles.
  - Real tests added to `tpt-t-cli` (`deny`/`profile` file writing,
    scaffold-then-build).
  - `tpt-t-cli doctor` — toolchain/environment sanity check.
  - `examples/` with three runnable example binaries.
  - `CHANGELOG.md` and `SECURITY.md`.
  - README "Getting Started" walkthrough, fixed CI badge, "no system dependencies"
    note.
- **Phase 17 — Innovative additions:**
  - `tpt-t-cli sim` — live terminal readout driving the Phase-4 simulator + safety
    loop via `PipelineHarness`/`QuadDrone`.
  - Live fleet dashboard — dependency-free static page at `GET /` on
    `tpt-t-cloud`, wired to the authenticated `/api/units/*` actions.
  - `tpt-t-cli replay` and `tpt-t-cli console` (MCP co-pilot) were added earlier
    in the Phase 17 work and are included here.
