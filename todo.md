# tpt-teleop — Project Roadmap

Tracks all work derived from `spec.txt`, ordered by build dependency/risk
(not spec section order). Simulation-first: Phase 4 builds a mock hardware
backend so Phases 5–8 can be built and tested before real hardware (Phase 9)
is wired in. Fully cross-platform (Linux/macOS/Windows) is in scope for v1.

## Phase 0 — Foundation & Tooling

Goal: repo, licensing, and CI exist before any feature code is written.

- [ ] Initialize Cargo workspace (`Cargo.toml` with `[workspace]` members)
- [ ] Create skeleton crate: `tpt-teleop-core`
- [ ] Create skeleton crate: `tpt-teleop-ring`
- [ ] Create skeleton crate: `tpt-teleop-link`
- [ ] Create skeleton crate: `tpt-teleop-input`
- [ ] Create skeleton crate: `tpt-teleop-media`
- [ ] Create skeleton crate: `tpt-teleop-safety`
- [ ] Create skeleton crate: `tpt-teleop-hal`
- [ ] Create skeleton crate: `tpt-teleop-cloud`
- [ ] Create skeleton crate: `tpt-teleop-sec`
- [ ] Create skeleton crate: `tpt-teleop-analytics`
- [ ] Create skeleton crate: `tpt-teleop-cli`
- [ ] Add `LICENSE-MIT` (TPT Solutions)
- [ ] Add `LICENSE-APACHE` (TPT Solutions)
- [ ] Set `license = "MIT OR Apache-2.0"` in workspace/crate `Cargo.toml` metadata
- [ ] Write `cargo-deny.toml`: allow MIT, BSD-2/3, ISC, Zlib, MPL-2.0
- [ ] `cargo-deny.toml`: force MIT resolution for dual MIT/Apache-2.0 crates
- [ ] `cargo-deny.toml`: ban strictly-Apache-2.0-only crates
- [ ] Set up CI: build + test matrix for Linux/macOS/Windows
- [ ] Set up CI: lint (clippy/fmt) job
- [ ] Set up CI: `cargo-deny check` job
- [ ] Write root `README.md` (project overview, license badges)
- [ ] Write architecture doc stub (crate map, data-flow diagram from spec §6)

## Phase 1 — Lock-Free Core (tpt-teleop-ring, tpt-teleop-core skeleton)

Goal: the zero-copy SPSC ring buffer and central state machine exist and are benchmarked.

- [ ] Design SPSC ring buffer layout (shared-memory allocation strategy)
- [ ] Implement lock-free/wait-free SPSC ring buffer
- [ ] Implement zero-copy pointer-passing API between producer/consumer
- [ ] Implement zero-copy struct-casting utilities (byte slice ↔ struct)
- [ ] Write unit tests for ring buffer correctness (single/multi producer-consumer pairs)
- [ ] Write throughput/latency benchmarks for ring buffer
- [ ] Implement central state machine skeleton in `tpt-teleop-core`
- [ ] Define Auto / Assist / Full-Teleop mode enum and state transitions
- [ ] Implement custom lock-free message bus in `tpt-teleop-core`

## Phase 2 — Cross-Platform Runtime (tpt-teleop-core)

Goal: a custom, no-async-runtime event loop with thread-per-core pinning on every target OS.

- [ ] Define event-loop abstraction trait (platform-agnostic)
- [ ] Implement Linux backend: `io_uring` event loop
- [ ] Implement macOS/BSD backend: `kqueue` event loop
- [ ] Implement Windows backend: custom IOCP event loop
- [ ] Implement thread-per-core pinning: Linux (`sched_setaffinity`)
- [ ] Implement thread-per-core pinning: macOS (thread affinity hints)
- [ ] Implement thread-per-core pinning: Windows (`SetThreadAffinityMask`)
- [ ] Define CPU core-pinning profile config format (e.g. Core 0 = video, Core 1 = control, Core 2 = network)

## Phase 3 — Zero-Copy Serialization

Goal: rkyv-based structs and helpers used by every subsystem downstream.

- [ ] Add `rkyv` dependency and configure workspace-wide serialization conventions
- [ ] Define `ControlCommand` struct with rkyv derive
- [ ] Define telemetry packet struct(s) with rkyv derive
- [ ] Implement zero-copy serialize helper (struct → pre-allocated buffer)
- [ ] Implement zero-copy deserialize helper (byte slice → struct cast)
- [ ] Implement pre-allocated serialization buffer pool

## Phase 4 — Mock Hardware / Simulator (tpt-teleop-hal sim backend)

Goal: full control loop can be built/tested with zero real hardware.

- [ ] Define HAL trait(s) covering motors, sensors, CAN bus, cameras
- [ ] Integrate `rapier` physics simulation
- [ ] Implement mock CAN bus backend
- [ ] Implement mock motor backend
- [ ] Implement mock sensor backend
- [ ] Build simulated robot/drone fixture for end-to-end integration tests

## Phase 5 — Safety & Autonomy Handover (tpt-teleop-safety)

Goal: deterministic safety loop intercepts and modifies commands in <10µs.

- [ ] Implement dedicated RT thread (Linux `SCHED_FIFO`)
- [ ] Implement equivalent RT/high-priority thread on macOS
- [ ] Implement equivalent RT/high-priority thread on Windows
- [ ] Implement geofencing logic
- [ ] Implement predictive collision avoidance (kinematic limit enforcement)
- [ ] Implement cubic-spline smoothed transitions between Auto/Assist/Teleop
- [ ] Implement latency compensation
- [ ] Implement emergency override intercept path
- [ ] Wire safety loop to pop from input ring, mutate in place, push to output ring
- [ ] Build <10µs intercept latency benchmark/test harness
- [ ] End-to-end test against Phase 4 simulator

## Phase 6 — Input (tpt-teleop-input)

Goal: controller/VR input flows zero-copy into the ring buffer pipeline.

- [ ] Implement raw HID polling backend: Linux (`hidapi` / custom `evdev` bindings)
- [ ] Implement raw HID polling backend: macOS
- [ ] Implement raw HID polling backend: Windows
- [ ] Implement universal controller abstraction layer
- [ ] Integrate OpenXR for 6DOF VR/AR hand tracking
- [ ] Implement haptic feedback / force-feedback routing
- [ ] Implement lock-free shared/co-pilot control state (multi-operator)
- [ ] Wire input subsystem: HID report → zero-copy cast → ring buffer → safety loop
- [ ] End-to-end test against Phase 4 simulator

## Phase 7 — Networking & Transport (tpt-teleop-link)

Goal: control/telemetry/WebRTC traffic multiplexed over a single UDP port with QUIC fallback.

- [ ] Implement custom UDP multiplexer (control + telemetry + WebRTC ICE on one port)
- [ ] Integrate `io_uring`-based async networking (Linux, via Phase 2 event loop)
- [ ] Integrate equivalent async networking on macOS/Windows (via Phase 2 event loop)
- [ ] Integrate `quinn` QUIC fallback for reliable control channel
- [ ] Design custom neighbor-discovery mesh protocol for drone swarms
- [ ] Implement mesh networking neighbor discovery
- [ ] Implement bandwidth throttling driven by real-time network backpressure
- [ ] Wire rkyv serialization directly into pre-allocated UDP packet buffers
- [ ] End-to-end test: safety loop output → link serialize → transmit

## Phase 8 — Media & Telemetry (tpt-teleop-media)

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

## Phase 9 — HAL Completion (tpt-teleop-hal real backends)

Goal: real hardware backends implemented behind the Phase 4 HAL trait.

- [ ] Implement raw SocketCAN backend (Linux, via `socketcan`)
- [ ] Research/implement cross-platform CAN backend (macOS/Windows)
- [ ] Implement custom MAVLink parser (from scratch, no `rust-mavlink`)
- [ ] Wire MAVLink parser to deserialize directly into rkyv structs
- [ ] Implement direct memory-mapped I/O for CAN bus and serial
- [ ] Validate real backends are drop-in swappable with Phase 4 mock backends

## Phase 10 — Cloud & Multi-Tenancy (tpt-teleop-cloud)

Goal: fleet management server and WebRTC SFU with no hyper/axum/tokio.

- [ ] Implement custom HTTP/3 server on `quinn` + `socket2`
- [ ] Implement fleet dashboard API endpoints
- [ ] Integrate `webrtc-rs`, patched to use custom lock-free ring buffers for media routing
- [ ] Implement WebRTC SFU media routing through the patched stack
- [ ] Implement session recording: raw rkyv byte streams written to disk

## Phase 11 — Security & Compliance (tpt-teleop-sec)

Goal: E2EE and zero-trust access control across link/cloud traffic.

- [ ] Integrate `ring` for AES-256-GCM
- [ ] Integrate `ring` for ChaCha20-Poly1305
- [ ] Implement zero-copy decrypt directly into `tpt-teleop-ring` buffer
- [ ] Implement zero-trust security model
- [ ] Implement RBAC (role-based access control)
- [ ] Integrate `tpt-teleop-sec` with `tpt-teleop-link` and `tpt-teleop-cloud`

## Phase 12 — Analytics & AI Export (tpt-teleop-analytics)

Goal: FDR logging never blocks the control loop; data is exportable for AI training.

- [ ] Implement O_DIRECT FDR logging (Linux, bypassing page cache)
- [ ] Implement equivalent direct-I/O logging on Windows (`FILE_FLAG_NO_BUFFERING`)
- [ ] Implement equivalent direct-I/O logging on macOS (`F_NOCACHE`)
- [ ] Implement AI training pipeline export: rkyv buffers → PyTorch-compatible format
- [ ] Implement AI training pipeline export: rkyv buffers → JAX-compatible format

## Phase 13 — Developer Experience & CLI

Goal: ergonomic macro-driven setup and project scaffolding tooling.

- [ ] Implement `#[tpt_teleop::robot(thread_per_core = true)]` proc-macro
- [ ] Implement `#[tpt_teleop::camera(...)]` field attribute macro
- [ ] Implement `#[tpt_teleop::motor(...)]` field attribute macro
- [ ] Macro codegen: generate lock-free rings from struct fields
- [ ] Macro codegen: generate thread-pinning setup from macro args
- [ ] Macro codegen: generate zero-copy serialization boilerplate
- [ ] Implement `tpt-teleop-cli` project scaffolding command
- [ ] Implement `tpt-teleop-cli` cargo-deny config generator
- [ ] Implement `tpt-teleop-cli` CPU core-pinning profile setup command

## Phase 14 — Integration, Benchmarking & Release

Goal: verified end-to-end, zero-allocation, zero-lock data path; ready to tag v1.0.0.

- [ ] Build full end-to-end test: Ingest → Normalize → Route → Safety → Serialize → Transmit
- [ ] Verify zero heap allocations in the hot path (allocation-counting tooling)
- [ ] Verify zero mutex locks in the hot path
- [ ] Run full cross-platform CI integration suite against Phase 4 simulator
- [ ] Build latency/jitter/throughput benchmark suite across all subsystems
- [ ] Write documentation pass + macro-driven quick-start guide
- [ ] Final license/cargo-deny audit before release
- [ ] Tag v1.0.0 release
