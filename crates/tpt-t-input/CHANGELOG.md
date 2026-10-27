# Changelog

All notable changes to `tpt-t-input` are documented here. The crate follows the
workspace's semantic versioning and is released with the workspace `vX.Y.Z`
tag. See the [workspace changelog](../../CHANGELOG.md) for the full project
history.

## [Unreleased]

## [1.0.0] — 2026-08-27

### Added
- Raw HID polling: Linux evdev over syscalls and Windows HID with overlapped reads.
- Universal controller abstraction and multi-operator state.
- OpenXR pose types and integration trait (null source until a runtime ships).
- Haptics routing.
- Lock-free AI input source with origin tagging and co-pilot arbitration.

### Known limitations
- macOS IOKit backend is a documented stub.
