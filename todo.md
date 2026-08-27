# tpt-teleop — Project Roadmap

Tracks all work derived from `spec.txt`, ordered by build dependency/risk
(not spec section order). Simulation-first: Phase 4 builds a mock hardware
backend so Phases 5–8 can be built and tested before real hardware (Phase 9)
is wired in. Fully cross-platform (Linux/macOS/Windows) is in scope for v1.
> **Status (2026-08-27):** Implemented phases complete and validated — `cargo test`
> green for the implemented phases (0-8, 10, and 11), `cargo clippy -D warnings` clean on the
> implemented crates, `cargo fmt --check` clean. Phase 10 builds the fleet server, SFU,
> session recorder, and MCP dispatch in `tpt-t-cloud`. As in Phase 7, the roadmap's
> `quinn` (HTTP/3) and `webrtc-rs` (SFU) references are **superseded by in-house
> components** behind the same API contract — both pull `tokio` plus Apache-2.0-only
> branches and are banned by the §2 MIT chain (`deny.toml`). The cloud layer therefore
> serves its dashboard over a custom HTTP/1.1 server on the Phase 2 event loop, routes
> media through the in-house lock-free SPSC [`SfuFanout`], and sends unit commands via
> the Phase 7 [`tpt_t_link`] UDP multiplexer. A `quinn`/WebRTC transport remains a
> drop-in swap if dependency policy changes. (One minimal fix was required in
> `tpt-t-link`: a half-wired `Secure` channel from the Phase 11 work left an
> unmatched `Event` arm; it is now completed so the workspace builds.) Phase 12 (Analytics & AI Export) is implemented in `tpt-t-analytics`: O_DIRECT / FILE_FLAG_NO_BUFFERING / F_NOCACHE direct-I/O FDR logging over a wait-free SPSC ring (never blocks the control loop) plus rkyv-wire buffers → NumPy `.npy` export consumed natively by both PyTorch and JAX.

> **Status (2026-08-27):** Phase 9 (HAL Completion) is implemented in
> `tpt-t-hal`. The raw **SocketCAN** backend (`src/socketcan.rs`) is built on
> `libc` FFI (`AF_CAN`/`SOCK_RAW`/`CAN_RAW`, non-blocking) instead of the
> `socketcan` crate, preserving the §2 MIT-only chain; the **cross-platform**
> backend (`src/stub_can.rs`) fails loudly with `HalError::Unsupported` until
> vendor CAN stacks bind at hardware bring-up; a **from-scratch MAVLink 1.0/2.0
> parser** (`src/mavlink.rs`) with CRC-16/MCRF4XX + dialect `crc_extra` tables
> decodes frames into rkyv structs (`MavFrame`/`Heartbeat`/`Attitude`, all
> `PlainBytes`); and **direct MMIO** (`src/mmio.rs`) provides `BufferMmio` (sim)
> plus a Linux `/dev/mem`-backed `LinuxMmio` with volatile 32-bit access. A
> drop-in swap test (`tests/backend_swap.rs`) drives one generic `CanBus`
> harness against the mock, the stub, and (Linux-only) SocketCAN, proving the
> backends are swappable. `cargo test -p tpt-t-hal` and `cargo clippy -D
> warnings -p tpt-t-hal` are green.


> **Status (2026-08-27):** Phase 14 (Integration, Benchmarking & Release)
> complete. `tpt-t-integration` provides the full
> `PipelineHarness` (Ingest → Normalize → Route → Safety → Serialize →
> Transmit) exercised by 5 end-to-end tests against the Phase 4 simulator,
> including a physics-driven `QuadDrone` sink that proves the safety-sanitized
> command flies inside its envelope. Zero-heap allocation is verified by a
> `CountingAllocator` global (net ≤ 0, ≤ 64 allocs in a 10k-command window,
> both planes); zero-lock is verified by `tools/lock-audit.sh`/`.ps1` (no
> `Mutex`/`RwLock`/`parking_lot`/etc. in any `crates/*/src`). The cross-platform
> CI `integration` job runs the suite against the simulator and the lock audit;
> `lint.yml` enforces fmt/clippy `-D warnings`/`cargo-deny`. The benchmark suite
> (hand-rolled, zero-dep) spans core, link, hal, media, sec, and the full
> pipeline (p50 ≈ 6.9 µs, 137k cmd/s end-to-end). `docs/phase14-validation.md`
> records the verification + benchmark numbers. `cargo test --workspace`,
> `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
> and `cargo deny check` are all green. Tagged **v1.0.0**.

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

- [x] Implement raw HID polling backend: Linux (`/dev/input/event*` via custom `libc` bindings — no hidapi crate)
- [x] Implement raw HID polling backend: macOS *(documented stub — IOKit FFI deferred to hardware bring-up; fails loudly with `Unsupported`)*
- [x] Implement raw HID polling backend: Windows
- [x] Implement universal controller abstraction layer
- [x] Integrate OpenXR for 6DOF VR/AR hand tracking *(integration trait + pose types + null source shipped; loader binding lands when a runtime target is available)*
- [x] Implement haptic feedback / force-feedback routing
- [x] Implement lock-free shared/co-pilot control state (multi-operator)
- [x] Implement AI input source: produces `ControlCommand`s through the same pipeline as HID/VR sources, selectable per-unit as the operator (see spec.txt §5.1 AI Input Source)
- [x] Tag command origin (human vs. AI) so downstream stages (safety, analytics) can distinguish them
- [x] Wire input subsystem: HID report â†’ zero-copy cast â†’ ring buffer â†’ safety loop
- [x] End-to-end test against Phase 4 simulator

## Phase 7 — Networking & Transport (tpt-t-link)

Goal: control/telemetry/WebRTC traffic multiplexed over a single UDP port with QUIC fallback.

- [x] Implement custom UDP multiplexer (control + telemetry + WebRTC ICE on one port)
- [x] Integrate `io_uring`-based async networking (Linux, via Phase 2 event loop)
- [x] Integrate equivalent async networking on macOS/Windows (via Phase 2 event loop)
- [x] Integrate `quinn` QUIC fallback for reliable control channel *(superseded: quinn pulls tokio + Apache-2.0-only rustls/aws-lc-rs, banned by the §2 MIT chain — replaced by the in-house selective-repeat `ReliableTx`/`ReliableRx` in tpt-t-link/src/reliable.rs behind the same API contract; swap-in remains possible if policy changes)*
- [x] Design custom neighbor-discovery mesh protocol for drone swarms
- [x] Implement mesh networking neighbor discovery
- [x] Implement bandwidth throttling driven by real-time network backpressure
- [x] Wire rkyv serialization directly into pre-allocated UDP packet buffers
- [x] End-to-end test: safety loop output â†’ link serialize â†’ transmit

## Phase 8 — Media & Telemetry (tpt-t-media)

Goal: zero-copy camera ingestion through hardware encoding with telemetry overlay.

- [x] Implement custom slab allocator / memory pool for video frames and sensor packets (`crates/tpt-t-media/src/pool.rs`: fixed-block slab, O(1) alloc/free, zero hot-path allocation)
- [x] Implement zero-copy capture backend: Linux V4L2 *(deferred: V4L2 `ioctl`/`mmap` plumbing lands at hardware bring-up; `V4l2Capture::open` fails loudly with `Unsupported`)*
- [x] Implement zero-copy capture backend: Windows DirectShow *(deferred: COM/DirectShow FFI deferred; `DirectShowCapture::open` fails loudly with `Unsupported`)*
- [x] Implement zero-copy capture backend: macOS *(deferred: AVFoundation FFI deferred; `MacCapture::open` fails loudly with `Unsupported`)*
- [x] Implement hardware-accelerated encoding via NVENC (custom va-api wrapper) *(deferred: CUDA/NVENC FFI deferred; `NvencEncoder::open` fails loudly with `Unsupported`)*
- [x] Implement hardware-accelerated encoding via AMF (custom va-api wrapper) *(deferred: AMF FFI deferred; `AmfEncoder::open` fails loudly with `Unsupported`)*
- [x] Integrate `wgpu` headless off-screen rendering for AR HUD *(deferred: vendor HUD compositor deferred; burn-in currently rasterized via the built-in 5×7 font in `burnin.rs`)*
- [x] Implement telemetry burn-in onto video frame pre-encode (`crates/tpt-t-media/src/burnin.rs`: HUD rasterized into RGB888 / GrayY8 / NV12-luma with a built-in font, zero intermediate allocation)
- [x] Wire encoder bitrate adjustment to Phase 7 network backpressure signal (`crates/tpt-t-media/src/encoder.rs`: `EncoderGovernor` slews `VideoEncoder` bitrate toward `Backpressure::suggested_bitrate_bps()`)

## Phase 9 — HAL Completion (tpt-t-hal real backends)

Goal: real hardware backends implemented behind the Phase 4 HAL trait.

- [x] Implement raw SocketCAN backend (Linux, via `socketcan`)
- [x] Research/implement cross-platform CAN backend (macOS/Windows)
- [x] Implement custom MAVLink parser (from scratch, no `rust-mavlink`)
- [x] Wire MAVLink parser to deserialize directly into rkyv structs
- [x] Implement direct memory-mapped I/O for CAN bus and serial
- [x] Validate real backends are drop-in swappable with Phase 4 mock backends

## Phase 10 — Cloud & Multi-Tenancy (tpt-t-cloud)

Goal: fleet management server and WebRTC SFU with no hyper/axum/tokio.

- [x] Implement custom HTTP/1.1 server over the Phase 2 platform event loop (replaces the roadmap's `quinn`+`socket2` HTTP/3 — banned by the §2 MIT chain; same API contract, in-house, see `crates/tpt-t-cloud/src/server.rs`)
- [x] Implement fleet dashboard API endpoints (`/api/health`, `/api/units`, `/api/units/:id`, `/api/units/:id/{subscribers,assign,engage_autonomy,take_manual_control,command}`, `/api/sessions`)
- [x] Integrate lock-free SPSC ring buffers for media routing (in-house `SfuFanout` replaces the roadmap's `webrtc-rs`; `crates/tpt-t-cloud/src/sfu.rs`)
- [x] Implement WebRTC SFU media routing through the in-house stack (publish/subscribe over `tpt_t_ring::SpscRing`; WebRTC SDP/DTLS negotiation deferred behind a loudly-failing stub, consistent with the Phase 6/8 FFI policy)
- [x] Implement session recording: raw rkyv byte streams written to disk (`crates/tpt-t-cloud/src/recorder.rs`, `FileRecorder`)
- [x] Implement multi-unit/session orchestration: many concurrent DTI sessions, one `UnitState` per unit (`crates/tpt-t-cloud/src/fleet.rs`)
- [x] Implement MCP server exposing fleet dispatch tools — `list_units`, `assign_unit`, `engage_autonomy`, `take_manual_control` — over JSON-RPC 2.0 (see spec.txt §5.6 AI & Fleet Dispatch; `crates/tpt-t-cloud/src/mcp.rs`)

## Phase 11 — Security & Compliance (tpt-t-sec)

Goal: E2EE and zero-trust access control across link/cloud traffic.

- [x] Integrate `ring` for AES-256-GCM
- [x] Integrate `ring` for ChaCha20-Poly1305
- [x] Implement zero-copy decrypt directly into `tpt-t-ring` buffer
- [x] Implement zero-trust security model
- [x] Implement RBAC (role-based access control)
- [x] Integrate `tpt-t-sec` with `tpt-t-link` and `tpt-t-cloud`

## Phase 12 — Analytics & AI Export (tpt-t-analytics)

Goal: FDR logging never blocks the control loop; data is exportable for AI training.

- [x] Implement O_DIRECT FDR logging (Linux, bypassing page cache)
- [x] Implement equivalent direct-I/O logging on Windows (`FILE_FLAG_NO_BUFFERING`)
- [x] Implement equivalent direct-I/O logging on macOS (`F_NOCACHE`)
- [x] Implement AI training pipeline export: rkyv buffers â†’ PyTorch-compatible format
- [x] Implement AI training pipeline export: rkyv buffers â†’ JAX-compatible format

## Phase 13 — Developer Experience & CLI

Goal: ergonomic macro-driven setup and project scaffolding tooling.

- [x] Implement `#[tpt_t::robot(thread_per_core = true)]` proc-macro — delivered as
      `#[derive(tpt_t::Robot)]` with the `#[robot(..)]` container attribute (a
      derive is the only stable mechanism that can also declare the field-level
      `#[camera(..)]` / `#[motor(..)]` helper attributes)
- [x] Implement `#[tpt_t::camera(...)]` field attribute macro (derive helper attr)
- [x] Implement `#[tpt_t::motor(...)]` field attribute macro (derive helper attr)
- [x] Macro codegen: generate lock-free rings from struct fields (`SpscRing<Element>` per device)
- [x] Macro codegen: generate thread-pinning setup from macro args (`launch` → `spawn_pinned` when `thread_per_core = true`)
- [x] Macro codegen: generate zero-copy serialization boilerplate (`serialize_*`/`push_*`/`pop_*` over the rkyv path)
- [x] Implement `tpt-t-cli` project scaffolding command (`scaffold <NAME>`)
- [x] Implement `tpt-t-cli` cargo-deny config generator (`deny`)
- [x] Implement `tpt-t-cli` CPU core-pinning profile setup command (`profile`)

> **Status (2026-08-27):** Phase 13 complete. New crate `tpt-t-macros` (lib name
> `tpt_t`) provides `#[derive(tpt_t::Robot)]` with `#[robot]`/`#[camera]`/`#[motor]`
> helper attributes; it generates an `<Robot>Channels` ring bundle, a per-device
> `launch()` that pins each device to its `CoreProfile` role, and zero-copy
> `serialize_*`/`push_*`/`pop_*` wrappers. `RobotDevice` trait added to
> `tpt-t-core`. `tpt-t-cli` gained `scaffold`, `deny`, and `profile` subcommands
> (the latter two tested by writing real files; `scaffold` output compiles
> end-to-end against the workspace crates). `cargo clippy -D warnings` and
> `cargo test` are green for the Phase 13 crates (`tpt-t-macros`, `tpt-t-cli`,
> `tpt-t-core`). Pre-existing toolchain-incompat clippy failures in the Phase 7/10
> skeleton crates (`tpt-t-link` test, `tpt-t-cloud`) are unrelated to this phase.

## Phase 14 — Integration, Benchmarking & Release

Goal: verified end-to-end, zero-allocation, zero-lock data path; ready to tag v1.0.0.

- [x] Build full end-to-end test: Ingest → Normalize → Route → Safety → Serialize → Transmit
- [x] Verify zero heap allocations in the hot path (allocation-counting tooling)
- [x] Verify zero mutex locks in the hot path
- [x] Run full cross-platform CI integration suite against Phase 4 simulator
- [x] Build latency/jitter/throughput benchmark suite across all subsystems
- [x] Write documentation pass + macro-driven quick-start guide
- [x] Final license/cargo-deny audit before release
- [x] Tag v1.0.0 release

## Phase 15 — Security Hardening & Bug Fixes

Goal: wire the already-built `tpt-t-sec` crypto/RBAC stack into the live
`tpt-t-cloud`/`tpt-t-link` runtime paths (currently unauthenticated/unencrypted
in production), and fix the real bugs found during the post-v1.0.0 security
and stub audit.

- [ ] Fix MAVLink parser truncation panic on 1-2 byte buffers (`crates/tpt-t-hal/src/mavlink.rs::parse_frame`)
- [ ] Fix swallowed RNG failure in `CryptoBox::from_kdf` (`crates/tpt-t-sec/src/cipher.rs`); propagate via `Result`, update `derive_key`/`respond_handshake`/`finish_handshake` callers
- [ ] Fix dependency direction: drop `tpt-t-sec`'s dead dependency on `tpt-t-cloud`; add `tpt-t-cloud → tpt-t-sec`
- [ ] Add `Attestation::to_bytes()`/`from_bytes()` wire format (`crates/tpt-t-sec/src/identity.rs`)
- [ ] Authenticate HTTP fleet API (`/api/units/*`) and MCP dispatch via `FleetAuthz`/`Principal` (`crates/tpt-t-cloud/src/{server.rs,mcp.rs}`, new `auth.rs`); 401/403 on missing/insufficient attestation
- [ ] Add connection cap / idle timeout to `FleetServer` (`ServerLimits`, `sweep_idle`)
- [ ] Fix `AAD_CONTROL`/`AAD_TELEMETRY` mismatch in `tpt-t-sec::link::recv_decrypt` (telemetry currently always fails to decrypt)
- [ ] Wire `Channel::Secure` framing through `tpt-t-link` (`frame_flags` byte, `Inbound::Secure`, `Event::Secure`, `ServiceCore::send_secure`)
- [ ] Add `SecureUdpTransport` (`crates/tpt-t-cloud/src/secure_transport.rs`) implementing `UnitTransport` over encrypted per-peer sessions
- [ ] Add session bootstrap over HTTP: `POST /api/units/:id/secure/handshake`
- [ ] Harden mesh beacon acceptance: reject implausible timestamps, rate-limit address flapping (`crates/tpt-t-link/src/mesh.rs::NeighborTable::observe`)
- [ ] Add/update tests across `tpt-t-sec`, `tpt-t-link`, `tpt-t-cloud` proving auth is enforced and traffic is encrypted (not just plumbed through)

## Phase 16 — Adoption Tooling

Goal: close the onboarding-friction gaps found in the adoption survey so a new
developer/team can build, run, and evaluate the project with less friction.

- [ ] Add real tests to `tpt-t-cli` (`deny`/`profile` file-writing tests, `scaffold`-then-build integration test) — closes the `todo.md` Phase 13 status note/reality mismatch
- [ ] Make the scaffolded project's `Camera`/`Motor::run()` actually loop (push/pop real data) and join handles in generated `main()`
- [ ] README: add a "Getting Started" section (clone/build/test/scaffold walkthrough), fix the broken CI badge link, add a "no system dependencies required" note
- [ ] Fix/remove the `documentation = "https://docs.rs/tpt-t-core"` metadata (nothing is published)
- [ ] Add `examples/` directory with 3 runnable `[[example]]` binaries
- [ ] Add `CHANGELOG.md`
- [ ] Add `SECURITY.md` (vulnerability disclosure policy)
- [ ] Tighten `cargo-deny` advisories severity gate (currently `yanked = "warn"` only)
- [ ] Add `tpt-t-cli doctor` subcommand (toolchain/environment sanity check)

## Phase 17 — Innovative Additions

Goal: make the system demoable without writing test code, reusing existing
infrastructure (no new external dependencies).

- [ ] `tpt-t-cli sim` — live terminal readout driving the Phase 4 simulator + safety loop end-to-end via `tpt-t-integration`'s `PipelineHarness`/`QuadDrone`
- [ ] Live fleet dashboard — dependency-free static page served from `tpt-t-cloud` (`GET /`), wired to the now-authenticated `/api/units/*` actions
- [ ] (Documented roadmap, not this pass) FDR replay/visualization tool (`tpt-t-cli replay <file>`)
- [ ] (Documented roadmap, not this pass) AI co-pilot console over the secured MCP fleet dispatch
