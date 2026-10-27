# tpt-t-cloud

Fleet management, multi-tenant cloud layer, and AI dispatch for
[tpt-teleop](../../README.md).

- **Fleet server** — an in-house HTTP/1.1 server with a live fleet dashboard
  at `GET /` and authenticated `/api/units/*` actions.
- **SFU** — selective-repeat media fan-out over lock-free SPSC rings.
- **Session recording** and multi-unit orchestration.
- **MCP dispatch** — JSON-RPC 2.0 so an AI client can drive the fleet.

Built with no `hyper`, `axum`, or `tokio`, and no Apache-only transport crates.
The workspace dependency policy (`deny.toml`) requires this, so the
`quinn`/WebRTC transports named in the roadmap are not used. A transport can be
swapped in behind the same traits if that policy changes.

## Usage

This crate is part of the tpt-teleop workspace and is not published to
crates.io. Depend on it by path or git:

```toml
[dependencies]
tpt-t-cloud = { path = "crates/tpt-t-cloud" }
```

Access is gated by `tpt-t-sec` attestation and RBAC: requests without a valid
attestation receive 401, and insufficient permissions receive 403.

## Changelog

See [CHANGELOG.md](CHANGELOG.md).

## License

Dual-licensed under MIT OR Apache-2.0, at your option. See the workspace
`LICENSE-MIT` and `LICENSE-APACHE`.
