# Changelog

All notable changes to `tpt-t-integration` are documented here. The crate
follows the workspace's semantic versioning and is released with the workspace
`vX.Y.Z` tag. See the [workspace changelog](../../CHANGELOG.md) for the full
project history.

## [Unreleased]

## [0.1.0] — 2026-08-27

### Added
- `PipelineHarness` for the full Ingest → Normalize → Route → Safety → Serialize → Transmit pipeline.
- End-to-end tests against the simulator, including a physics-driven `QuadDrone` sink.
- Zero-allocation verification via a `CountingAllocator` global.
- Zero-lock verification via `tools/lock-audit.sh` and `tools/lock-audit.ps1`.
- Cross-subsystem benchmark suite.
- Examples: `pipeline_demo`, `sim_drone`, `safety_clamp`.
