# Security Policy

## Supported Versions

| Version | Supported |
|---------|-----------|
| 1.1.x   | ✅ |
| 1.0.x   | ✅ (security fixes only) |
| < 1.0   | ❌ |

## Reporting a Vulnerability

We take the security of tpt-teleop seriously. Because this middleware carries
real-time safety-critical commands to physical robots and drones, please report
any vulnerability **privately** rather than opening a public issue.

### How to report
- **GitHub Security Advisories (preferred):** use the
  [Report a vulnerability](https://github.com/tpt-solutions/tpt-teleop/security/advisories/new)
  form on the repository. This keeps the report confidential until a fix is
  released.
- **Email:** security@tpt.solutions (PGP encouraged).

Please include:
- A description of the vulnerability and its impact (esp. on the safety loop or
  command/telemetry integrity).
- Affected version(s) and platform(s) (Linux/macOS/Windows).
- Steps to reproduce, or a proof-of-concept if possible.

### What to expect
- **Acknowledgement** within 5 business days.
- **Triage and severity assessment** within 10 business days.
- **Coordinated disclosure:** we will work with you on a fix and a release date,
  and will credit you in `CHANGELOG.md` unless you prefer to remain anonymous.
  We ask that public disclosure wait until a patched release is available.

## Security Model

tpt-teleop is built around a zero-trust posture:
- **Encryption:** all unit traffic is sealed with `ring`-backed
  AES-256-GCM / ChaCha20-Poly1305 via per-peer `CryptoBox` sessions (see
  `tpt-t-sec`).
- **Authentication:** fleet HTTP API and MCP dispatch are gated by
  `FleetAuthz`/`Principal`; missing or insufficient attestation yields `401`/`403`.
- **Authorization:** RBAC (`tpt-t-sec::rbac`) scopes tools such as
  `engage_autonomy` / `take_manual_control` to the appropriate roles.
- **Dependency policy:** the "MIT chain" (`deny.toml`) forbids Apache-2.0-only
  crates (which carry patent-liability risk) and enforces license/advisory audits
  in CI.

## Scope notes
Out-of-scope for this policy are the documented "loudly failing" hardware FFI
backends (V4L2, SocketCAN, DirectShow, NVENC, AVFoundation) that are stubbed
until hardware bring-up, and the unauthenticated `CapturingTransport`/`NullTransport`
test doubles — these are development/test aids, not production paths.
