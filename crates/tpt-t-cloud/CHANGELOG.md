# Changelog

All notable changes to `tpt-t-cloud` are documented here. The crate follows the
workspace's semantic versioning and is released with the workspace `vX.Y.Z`
tag. See the [workspace changelog](../../CHANGELOG.md) for the full project
history.

## [Unreleased]

## [1.1.0] — 2026-08-28

### Added
- Authenticated fleet API: `FleetAuthz` and `Principal` return 401 or 403 on
  missing or insufficient attestation (Phase 15).
- Connection cap and idle sweep on `FleetServer` (Phase 15).
- Live fleet dashboard at `GET /`, wired to the `/api/units/*` actions (Phase 17).

## [0.1.0] — 2026-08-27

### Added
- In-house HTTP/1.1 fleet server, with no `hyper`, `axum`, or `tokio`.
- `SfuFanout` media routing over lock-free SPSC rings.
- Session recorder and multi-unit orchestration.
- MCP dispatch over JSON-RPC 2.0.
