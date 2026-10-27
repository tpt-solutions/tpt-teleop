# tpt-t-sec

Zero-trust security for [tpt-teleop](../../README.md): per-unit identity,
role-based access control, and end-to-end encryption.

- **Cipher** — AES-256-GCM and ChaCha20-Poly1305 with zero-copy seal and open
  paths. Pure-Rust RustCrypto implementations, so there is no C build step.
- **Identity** — Ed25519 signing keys and signed attestations that bind a
  session to an enrolled key.
- **Session** — mutually authenticated, forward-secret key agreement (ephemeral
  X25519 ECDH with HKDF).
- **RBAC** — roles, permissions, and a per-action policy.
- **Zero-trust broker** — every request is verified and every privileged action
  is re-checked. Nothing is trusted by network position.
- **Integrations** — `SecureMux` for `tpt-t-link` and `FleetAuthz` for
  `tpt-t-cloud`.

## Usage

This crate is part of the tpt-teleop workspace and is not published to
crates.io. Depend on it by path or git:

```toml
[dependencies]
tpt-t-sec = { path = "crates/tpt-t-sec" }
```

Run the crypto benchmark with `cargo bench -p tpt-t-sec`.

## Security

To report a vulnerability, see the workspace [SECURITY.md](../../SECURITY.md).

## Changelog

See [CHANGELOG.md](CHANGELOG.md).

## License

Dual-licensed under MIT OR Apache-2.0, at your option. See the workspace
`LICENSE-MIT` and `LICENSE-APACHE`.
