# Changelog

All notable changes to `tpt-t-analytics` are documented here. The crate
follows the workspace's semantic versioning and is released with the
workspace `vX.Y.Z` tag. See the [workspace changelog](../../CHANGELOG.md) for
the full project history.

## [Unreleased]

## [0.1.0] — 2026-08-27

### Added
- Direct-I/O flight data recorder (`O_DIRECT`, `FILE_FLAG_NO_BUFFERING`,
  `F_NOCACHE`) fed by a wait-free SPSC ring, so logging never blocks the control loop.
- Offline export of recorded data to NumPy `.npy` arrays for PyTorch and JAX.
