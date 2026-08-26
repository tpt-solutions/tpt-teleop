# tpt-teleop — Project Roadmap

Tracks all work derived from `spec.txt`, ordered by build dependency/risk
(not spec section order). Simulation-first: Phase 4 builds a mock hardware
backend so Phases 5–8 can be built and tested before real hardware (Phase 9)
is wired in. Fully cross-platform (Linux/macOS/Windows) is in scope for v1.
> **Status (2026-08-26):** Phases 0-5 complete and validated — `cargo test` green (workspace); `cargo clippy -D warnings` clean on Windows, x86_64-linux-gnu and aarch64-apple-darwin targets; `cargo fmt --check` clean. See docs/ARCHITECTURE.md for the implemented design.


## Phase 0 — Foundation & Tooling

Goal: repo, licensing, and CI exist before any feature code is written.

- [x] Initialize Cargo workspace (`Cargo.toml` with `[workspace]` members)
- [x] Create skeleton crate: `tpt-t-core`
- [x] Create skeleton crate: `tpt-t-ring`
- [x] Create skeleton crate: `tpt-t-link`
- [x] Create skeleton crate: `tpt-t-input`
- [x] Create skeleton crate: `tpt-t-media`
- [x] Create skeleton crate: `tpt-t-safety`
- [x] Create skeleton crate: `tpt-t-hal`
- [x] Create skeleton crate: `tpt-t-cloud`
- [x] Create skeleton crate: `tpt-t-sec`
- [x] Create skeleton crate: `tpt-t-analytics`
- [x] Create skeleton crate: `tpt-t-cli`
- [x] Add `LICENSE-MIT` (TPT Solutions)
- [x] Add `LICENSE-APACHE` (TPT Solutions)
- [x] Set `license = "MIT OR Apache-2.0"` in workspace/crate `Cargo.toml` metadata
- [x] Write `cargo-deny.toml`: allow MIT, BSD-2/3, ISC, Zlib, MPL-2.0
- [x] `cargo-deny.toml`: force MIT resolution for dual MIT/Apache-2.0 crates
- [x] `cargo-deny.toml`: ban strictly-Apache-2.0-only crates
- [x] Set up CI: build + test matrix for Linux/macOS/Windows
- [x] Set up CI: lint (clippy/fmt) job
- [x] Set up CI: `cargo-deny check` job
- [x] Write root `README.md` (project overview, license badges)
- [x] Write architecture doc stub (crate map, data-flow diagram from spec Â§6)

## Phase 1 — Lock-Free Core (tpt-t-ring, tpt-t-core skeleton)

Goal: the zero-copy SPSC ring buffer and central state machine exist and are benchmarked.

- [x] Design SPSC ring buffer layout (shared-memory allocation strategy)
- [x] Implement lock-free/wait-free SPSC ring buffer
- [x] Implement zero-copy pointer-passing API between producer/consumer
- [x] Implement zero-copy struct-casting utilities (byte slice â†” struct)
- [x] Write unit tests for ring buffer correctness (single/multi producer-consumer pairs)
- [x] Write throughput/latency benchmarks for ring buffer
- [x] Implement central state machine skeleton in `tpt-t-core`
- [x] Define Auto / Assist / Full-Teleop mode enum and state transitions
- [x] Implement custom lock-free message bus in `tpt-t-core`

## Phase 2 — Cross-Platform Runtime (tpt-t-core)

Goal: a custom, no-async-runtime event loop with thread-per-core pinning on every target OS.

- [x] Define event-loop abstraction trait (platform-agnostic)
- [x] Implement Linux backend: `io_uring` event loop *(shipped as the custom epoll loop spec §3.1 explicitly permits; io_uring zero-copy transmit lands in tpt-t-link, Phase 7)*
- [x] Implement macOS/BSD backend: `kqueue` event loop
- [x] Implement Windows backend: custom IOCP event loop
- [x] Implement thread-per-core pinning: Linux (`sched_setaffinity`)
- [x] Implement thread-per-core pinning: macOS (thread affinity hints)
- [x] Implement thread-per-core pinning: Windows (`SetThreadAffinityMask`)
- [x] Define CPU core-pinning profile config format (e.g. Core 0 = video, Core 1 = control, Core 2 = network)

## Phase 3 — Zero-Copy Serialization

Goal: rkyv-based structs and helpers used by every subsystem downstream.

- [x] Add `rkyv` dependency and configure workspace-wide serialization conventions
- [x] Define `ControlCommand` struct with rkyv derive
- [x] Define telemetry packet struct(s) with rkyv derive
- [x] Implement zero-copy serialize helper (struct â†’ pre-allocated buffer)
- [x] Implement zero-copy deserialize helper (byte slice â†’ struct cast)
- [x] Implement pre-allocated serialization buffer pool

## Phase 4 — Mock Hardware / Simulator (tpt-t-hal sim backend)

Goal: full control loop can be built/tested with zero real hardware.

- [x] Define HAL trait(s) covering motors, sensors, CAN bus, cameras
- [x] Integrate `rapier` physics simulation *(superseded: rapier ships Apache-2.0-only, banned by the §2 cargo-deny MIT chain — replaced by a deterministic in-house rigid-body core in tpt-t-hal::sim::world; swap-in remains possible behind the same fixture API if policy changes)*
- [x] Implement mock CAN bus backend
- [x] Implement mock motor backend
- [x] Implement mock sensor backend
- [x] Build simulated robot/drone fixture for end-to-end integration tests

## Phase 5 — Safety & Autonomy Handover (tpt-t-safety)

Goal: deterministic safety loop intercepts and modifies commands in <10Âµs.

- [x] Implement dedicated RT thread (Linux `SCHED_FIFO`)
- [x] Implement equivalent RT/high-priority thread on macOS
- [x] Implement equivalent RT/high-priority thread on Windows
- [x] Implement geofencing logic
- [x] Implement predictive collision avoidance (kinematic limit enforcement)
- [x] Implement cubic-spline smoothed transitions between Auto/Assist/Teleop
- [x] Implement latency compensation
- [x] Implement emergency override intercept path
- [x] Implement override/veto arbitration: clamp/restrict a human-authored `ControlCommand` when an AI input source is also present on the unit, without the AI ever injecting new intent (see spec.txt §5.4 Shared Control)
- [x] Wire safety loop to pop from input ring, mutate in place, push to output ring
- [x] Build <10Âµs intercept latency benchmark/test harness
- [x] End-to-end test against Phase 4 simulator

## Phase 6 — Input (tpt-t-input)

Goal: controller/VR input flows zero-copy into the ring buffer pipeline.

- [ ] Implement raw HID polling backend: Linux (`hidapi` / custom `evdev` bindings)
- [ ] Implement raw HID polling backend: macOS
- [ ] Implement raw HID polling backend: Windows
- [ ] Implement universal controller abstraction layer
- [ ] Integrate OpenXR for 6DOF VR/AR hand tracking
- [ ] Implement haptic feedback / force-feedback routing
- [ ] Implement lock-free shared/co-pilot control state (multi-operator)
- [ ] Implement AI input source: produces `ControlCommand`s through the same pipeline as HID/VR sources, selectable per-unit as the operator (see spec.txt §5.1 AI Input Source)
- [ ] Tag command origin (human vs. AI) so downstream stages (safety, analytics) can distinguish them
- [ ] Wire input subsystem: HID report â†’ zero-copy cast â†’ ring buffer â†’ safety loop
- [ ] End-to-end test against Phase 4 simulator

## Phase 7 — Networking & Transport (tpt-t-link)

Goal: control/telemetry/WebRTC traffic multiplexed over a single UDP port with QUIC fallback.

- [ ] Implement custom UDP multiplexer (control + telemetry + WebRTC ICE on one port)
- [ ] Integrate `io_uring`-based async networking (Linux, via Phase 2 event loop)
- [ ] Integrate equivalent async networking on macOS/Windows (via Phase 2 event loop)
- [ ] Integrate `quinn` QUIC fallback for reliable control channel
- [ ] Design custom neighbor-discovery mesh protocol for drone swarms
- [ ] Implement mesh networking neighbor discovery
- [ ] Implement bandwidth throttling driven by real-time network backpressure
- [ ] Wire rkyv serialization directly into pre-allocated UDP packet buffers
- [ ] End-to-end test: safety loop output â†’ link serialize â†’ transmit

## Phase 8 — Media & Telemetry (tpt-t-media)

Goal: zero-copy camera ingestion through hardware encoding with telemetry overlay.

- [ ] Implement custom slab allocator / memory pool for video frames and sensor packets
- [ ] Implement zero-copy capture backend: Linux V4L2
- [ ] Implement zero-copy capture backend: Windows DirectShow
- [ ] Implement zero-copy capture backend: macOS
- [ ] Implement hardware-accelerated encoding via NVENC (custom va-api wrapper)
- [ ] Implement hardware-accelerated encoding via AMF (custom va-api wrapper)
- [ ] Integrate `wgpu` headless off-screen rendering for AR HUD
- [ ] Implement telemetry burn-in onto video frame pre-encode
- [ ] Wire encoder bitrate adjustment to Phase 7 network backpressure signal

## Phase 9 — HAL Completion (tpt-t-hal real backends)

Goal: real hardware backends implemented behind the Phase 4 HAL trait.

- [ ] Implement raw SocketCAN backend (Linux, via `socketcan`)
- [ ] Research/implement cross-platform CAN backend (macOS/Windows)
- [ ] Implement custom MAVLink parser (from scratch, no `rust-mavlink`)
- [ ] Wire MAVLink parser to deserialize directly into rkyv structs
- [ ] Implement direct memory-mapped I/O for CAN bus and serial
- [ ] Validate real backends are drop-in swappable with Phase 4 mock backends

## Phase 10 — Cloud & Multi-Tenancy (tpt-t-cloud)

Goal: fleet management server and WebRTC SFU with no hyper/axum/tokio.

- [ ] Implement custom HTTP/3 server on `quinn` + `socket2`
- [ ] Implement fleet dashboard API endpoints
- [ ] Integrate `webrtc-rs`, patched to use custom lock-free ring buffers for media routing
- [ ] Implement WebRTC SFU media routing through the patched stack
- [ ] Implement session recording: raw rkyv byte streams written to disk
- [ ] Implement multi-unit/session orchestration: many concurrent DTI sessions, one per unit
- [ ] Implement MCP server exposing fleet dispatch tools (list units, assign unit, engage autonomy, take manual control) (see spec.txt §5.6 AI & Fleet Dispatch)

## Phase 11 — Security & Compliance (tpt-t-sec)

Goal: E2EE and zero-trust access control across link/cloud traffic.

- [ ] Integrate `ring` for AES-256-GCM
- [ ] Integrate `ring` for ChaCha20-Poly1305
- [ ] Implement zero-copy decrypt directly into `tpt-t-ring` buffer
- [ ] Implement zero-trust security model
- [ ] Implement RBAC (role-based access control)
- [ ] Integrate `tpt-t-sec` with `tpt-t-link` and `tpt-t-cloud`

## Phase 12 — Analytics & AI Export (tpt-t-analytics)

Goal: FDR logging never blocks the control loop; data is exportable for AI training.

- [ ] Implement O_DIRECT FDR logging (Linux, bypassing page cache)
- [ ] Implement equivalent direct-I/O logging on Windows (`FILE_FLAG_NO_BUFFERING`)
- [ ] Implement equivalent direct-I/O logging on macOS (`F_NOCACHE`)
- [ ] Implement AI training pipeline export: rkyv buffers â†’ PyTorch-compatible format
- [ ] Implement AI training pipeline export: rkyv buffers â†’ JAX-compatible format

## Phase 13 — Developer Experience & CLI

Goal: ergonomic macro-driven setup and project scaffolding tooling.

- [ ] Implement `#[tpt_t::robot(thread_per_core = true)]` proc-macro
- [ ] Implement `#[tpt_t::camera(...)]` field attribute macro
- [ ] Implement `#[tpt_t::motor(...)]` field attribute macro
- [ ] Macro codegen: generate lock-free rings from struct fields
- [ ] Macro codegen: generate thread-pinning setup from macro args
- [ ] Macro codegen: generate zero-copy serialization boilerplate
- [ ] Implement `tpt-t-cli` project scaffolding command
- [ ] Implement `tpt-t-cli` cargo-deny config generator
- [ ] Implement `tpt-t-cli` CPU core-pinning profile setup command

## Phase 14 — Integration, Benchmarking & Release

Goal: verified end-to-end, zero-allocation, zero-lock data path; ready to tag v1.0.0.

- [ ] Build full end-to-end test: Ingest â†’ Normalize â†’ Route â†’ Safety â†’ Serialize â†’ Transmit
- [ ] Verify zero heap allocations in the hot path (allocation-counting tooling)
- [ ] Verify zero mutex locks in the hot path
- [ ] Run full cross-platform CI integration suite against Phase 4 simulator
- [ ] Build latency/jitter/throughput benchmark suite across all subsystems
- [ ] Write documentation pass + macro-driven quick-start guide
- [ ] Final license/cargo-deny audit before release
- [ ] Tag v1.0.0 release
