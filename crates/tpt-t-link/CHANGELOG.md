# Changelog

All notable changes to `tpt-t-link` are documented here. The crate follows the
workspace's semantic versioning and is released with the workspace `vX.Y.Z`
tag. See the [workspace changelog](../../CHANGELOG.md) for the full project
history.

## [Unreleased]

## [1.1.0] — 2026-08-28

### Changed
- `Channel::Secure` framing is wired end to end, and the AAD mismatch is fixed
  so telemetry decrypts under its own AAD (Phase 15).
- Mesh beacons reject implausible timestamps and rate-limit address flapping (Phase 15).
- `SecureUdpTransport` runs encrypted per-peer sessions, bootstrapped through
  the cloud handshake endpoint (Phase 15).

## [0.1.0] — 2026-08-27

### Added
- Custom UDP multiplexer on one port, with control, telemetry, media, ICE, and mesh channels.
- Service loop on the platform event loop, with one pinned network thread.
- io_uring transmit path (Linux).
- Neighbor-discovery mesh and backpressure throttling.
- In-house selective-repeat reliable transport.
