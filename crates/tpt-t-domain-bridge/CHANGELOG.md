# Changelog

All notable changes to `tpt-t-domain-bridge` are documented here. The crate
follows the workspace's semantic versioning and is released with the workspace
`vX.Y.Z` tag. See the [workspace changelog](../../CHANGELOG.md) for the full
project history.

## [Unreleased]

### Changed (breaking)
- `DomainError::Adapter` now holds a `&'static str` instead of a `String`, and
  `DomainError::adapter` takes `&'static str`. Building an adapter error no
  longer allocates, so the DTI hooks stay allocation-free on their error paths.
  Adapters that formatted dynamic messages must use a fixed message instead.

## [1.0.0] — 2026-08-27

### Added
- Domain Teleoperation Interface: the `DomainTeleopInterface` adapter trait,
  `OperatorId`, `DomainError`, and rkyv wire types.
- Autonomy ↔ teleop handover lifecycle and its mapping onto the core `Mode`.
- `DomainSession` driver that enforces the lifecycle.
