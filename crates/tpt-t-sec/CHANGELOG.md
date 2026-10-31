# Changelog

All notable changes to `tpt-t-sec` are documented here. The crate follows the
workspace's semantic versioning and is released with the workspace `vX.Y.Z`
tag. See the [workspace changelog](../../CHANGELOG.md) for the full project
history.

## [Unreleased]

## [1.1.0] — 2026-08-28

### Changed
- Crypto and RBAC are live in `tpt-t-cloud` and `tpt-t-link` (Phase 15).
- `FleetAuthz` and `Principal` gate the fleet API and MCP dispatch.
- `SecureMux` is integrated with the link layer.

## [0.1.0] — 2026-08-27

### Added
- AEAD ciphers: AES-256-GCM and ChaCha20-Poly1305 (RustCrypto).
- Ed25519 identities and signed attestations.
- Forward-secret session key agreement (X25519 ECDH with HKDF).
- Zero-trust broker and RBAC policy.
