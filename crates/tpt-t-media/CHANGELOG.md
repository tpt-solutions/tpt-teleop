# Changelog

All notable changes to `tpt-t-media` are documented here. The crate follows the
workspace's semantic versioning and is released with the workspace `vX.Y.Z`
tag. See the [workspace changelog](../../CHANGELOG.md) for the full project
history.

## [Unreleased]

## [1.0.0] — 2026-08-27

### Added
- Slab allocator for video frames and sensor packets, with O(1) alloc and free.
- Telemetry burn-in HUD rasterized before encode.
- `EncoderGovernor` that slews bitrate toward the link backpressure signal.
- `NullEncoder` and `TestPatternCapture` for simulation and tests.

### Known limitations
- Hardware capture and encode backends (V4L2, DirectShow, AVFoundation, NVENC,
  AMF) return `MediaError::Unsupported` until hardware bring-up.
