# Changelog

All notable changes to `tpt-t-hal` are documented here. The crate follows the
workspace's semantic versioning and is released with the workspace `vX.Y.Z`
tag. See the [workspace changelog](../../CHANGELOG.md) for the full project
history.

## [Unreleased]

## [1.0.0] — 2026-08-27

### Added
- HAL traits for motors, sensors, CAN bus, and cameras.
- Mock backends: virtual CAN over SPSC rings, deterministic ESC models, null camera.
- `QuadDrone` physics simulator fixture.
- Raw SocketCAN backend (Linux, `libc` FFI). Other platforms return `HalError::Unsupported`.
- From-scratch MAVLink 1.0/2.0 parser with CRC-16/MCRF4XX and dialect tables.
- MMIO access: `BufferMmio` for simulation and `LinuxMmio` over `/dev/mem`.
