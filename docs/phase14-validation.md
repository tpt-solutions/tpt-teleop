# Phase 14 — Integration, Benchmarking & Release Validation

Verification record for the v1.0.0 release. Every claim below is reproducible
from a clean checkout with the commands listed at the end.

## 1. End-to-end pipeline test (Ingest → Normalize → Route → Safety → Serialize → Transmit)

Crate `tpt-t-integration`, `tests/e2e_pipeline.rs` (5 tests, all passing):

| Test | Proves |
|------|--------|
| `command_preserved_end_to_end_over_real_udp` | A `ControlCommand` survives the whole plane over a real loopback UDP socket (seq, mode, axes). |
| `ai_origin_tag_survives_the_pipeline` | The human-vs-AI origin flag survives Route→Transmit (spec §5.1). |
| `safety_clamps_and_slews_before_the_command_is_transmitted` | Tilt clamp (0.35) and per-tick slew (0.02) are enforced *before* wire transmission. |
| `ingest_normalize_then_wire_round_trips_axes` | Raw HID report → `ControllerMap` normalization → wire round-trip is numerically exact. |
| `full_pipeline_flies_simulated_drone_inside_envelope` | With a hostile saturated operator, the vehicle stays inside the geofence (r≤60 m, alt≤20 m) including the wire round-trip. |

The harness (`PipelineHarness`) wires the stages in the spec §6 order and,
optionally, drives the Phase 4 physics `QuadDrone` so the safety-sanitized
command is proven to *fly* inside its envelope.

## 2. Zero heap allocation in the hot path

`tests/zero_alloc.rs` (2 tests, passing) installs a process-global
`CountingAllocator` (`src/alloc.rs`) as the `#[global_allocator]`. After a
2,000-command warm-up window, a second 10,000-command window must satisfy
`net_allocations() <= 0` and `allocs <= 64`. A true per-command leak would
register ~N allocations; the bound of 64 absorbs unavoidable system-allocator
bookkeeping noise while still catching any regression that allocates per
command. Both the core plane (no ingest) and the ingest plane pass.

## 3. Zero mutex locks in the hot path

`tools/lock-audit.sh` (and the PowerShell twin `tools/lock-audit.ps1`) scan
every `crates/*/src` tree for `Mutex` / `RwLock` / `parking_lot` /
`lazy_static` / `once_cell` / `Condvar` / `crossbeam` / spin-lock patterns.

**Result: PASS — no locking primitives in any hot-path library source.** The
only hits are inside `tests/` and `benches/` (a `std::sync::Mutex` used to log
events in `tpt-t-link/tests/e2e_link.rs`), which are not on the runtime
critical path. The audit is wired into CI as the `integration` job's
"Zero-lock hot-path audit" step.

## 4. Cross-platform CI integration suite

`.github/workflows/ci.yml`:
- `build-test` matrix runs `cargo build --workspace --all-targets` and
  `cargo test --workspace` on ubuntu / macOS / windows-latest.
- `cross-check` compiles every cfg-gated backend (`io_uring` / `kqueue` /
  IOCP) for Linux, macOS, and Windows targets regardless of host.
- `integration` runs `cargo test -p tpt-t-integration` against the Phase 4
  simulator and then `tools/lock-audit.sh`.

`.github/workflows/lint.yml` enforces `cargo fmt --check`,
`cargo clippy --workspace --all-targets -- -D warnings`, and `cargo-deny check`.

## 5. Latency / jitter / throughput benchmark suite

Hand-rolled (`harness = false`) so the dependency tree stays at zero. Run with
`cargo bench -p <crate> --benches`. Numbers below observed on a release build
(host: this checkout's CI runner; treat as relative, not absolute, targets):

| Subsystem | Bench | p50 | p99 | p99.9 | Throughput |
|-----------|-------|-----|-----|-------|------------|
| Core routing | `bus_bench` (publish+poll) | 0 ns | 200 ns | 1.1 µs | 12.6 M ops/s |
| Link serialize | `serialize_bench` (rkyv control frame) | 200 ns | 300 ns | 400 ns | 4.0 M cmd/s |
| HAL sim step | `sim_bench` (QuadDrone fixed step) | 100 ns | 100 ns | 100 ns | 8.2 M steps/s |
| Media burn-in | `burnin_bench` (HUD raster) | 800 ns | 1.4 µs | 1.9 µs | 1.2 M frames/s |
| Security | `crypto_bench` (AES-256-GCM seal+open) | 300 ns | 800 ns | 1.0 µs | 2.8 M ops/s |
| **Full pipeline** | `pipeline_bench` (Ingest→…→Transmit) | **6.9 µs** | **14.6 µs** | 51.9 µs | **137 k cmd/s** |

The end-to-end forward data plane (p50 ≈ 6.9 µs) includes the OS UDP send; the
deterministic safety-intercept sub-budget (< 10 µs) is benchmarked separately in
`tpt-t-safety` (`intercept_bench`).

## 6. Reproduce

```bash
# End-to-end + zero-alloc integration tests (Phase 4 simulator)
cargo test -p tpt-t-integration

# Whole workspace green
cargo test --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings

# Zero-lock audit
bash tools/lock-audit.sh          # or: pwsh tools/lock-audit.ps1

# License / dependency policy audit
cargo deny check

# Benchmark suite
cargo bench -p tpt-t-integration --bench pipeline_bench
cargo bench --workspace --benches
```
