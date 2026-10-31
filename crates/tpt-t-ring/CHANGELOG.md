# Changelog

All notable changes to `tpt-t-ring` are documented here. The crate follows the
workspace's semantic versioning and is released with the workspace `vX.Y.Z`
tag. See the [workspace changelog](../../CHANGELOG.md) for the full project
history.

## [Unreleased]

## [0.1.0] — 2026-08-27

### Added
- `SpscRing`: a wait-free, bounded, heap-backed SPSC queue.
- `shared::SharedSpsc` for caller-provided memory, enabling cross-process IPC.
- Pointer-passing rings for zero-copy buffer hand-off.
- Zero-copy byte-to-struct casting for `#[repr(C)]` types.
- Deterministic shared-memory layout math.
