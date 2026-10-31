# Changelog

All notable changes to `tpt-t-cli` are documented here. The crate follows the
workspace's semantic versioning and is released with the workspace `vX.Y.Z`
tag. See the [workspace changelog](../../CHANGELOG.md) for the full project
history.

## [Unreleased]

## [1.1.0] — 2026-08-28

### Added
- `doctor` — toolchain and environment sanity check (Phase 16).
- `scaffold` output compiles end to end, verified by a scaffold-then-build test (Phase 16).
- `sim` — live terminal readout driving the simulator and safety loop (Phase 17).
- `replay` — FDR replay (Phase 17).
- `console` — MCP fleet-dispatch console (Phase 17).

## [0.1.0] — 2026-08-27

### Added
- `scaffold` — new robot project generation.
- `deny` — cargo-deny MIT-chain config generation.
- `profile` — CPU core-pinning profile generation.
