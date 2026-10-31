# Changelog

All notable changes to `tpt-t-core` are documented here. The crate follows the
workspace's semantic versioning and is released with the workspace `vX.Y.Z`
tag. See the [workspace changelog](../../CHANGELOG.md) for the full project
history.

## [Unreleased]

## [0.1.0] — 2026-08-27

### Added
- Central state machine and atomic autonomy transition engine.
- Lock-free message bus and bounded MPMC queue.
- Pre-allocated buffer pool with zero steady-state allocation.
- rkyv wire types (`ControlCommand`, telemetry) with zero-copy serialization.
- Platform event loops: io_uring (Linux), kqueue (macOS), and IOCP (Windows).
- Thread-per-core pinning and core-pinning profiles.
