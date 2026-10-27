# Changelog

All notable changes to `tpt-t-macros` are documented here. The crate follows
the workspace's semantic versioning and is released with the workspace `vX.Y.Z`
tag. See the [workspace changelog](../../CHANGELOG.md) for the full project
history.

## [Unreleased]

## [1.0.0] — 2026-08-27

### Added
- `#[derive(Robot)]` with `#[robot]`, `#[camera]`, and `#[motor]` attributes.
- Codegen for lock-free rings, CPU-pinned device launch, and zero-copy
  serialize, push, and pop wrappers.
