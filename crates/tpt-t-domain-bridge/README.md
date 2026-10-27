# tpt-t-domain-bridge

The Domain Teleoperation Interface (DTI) for [tpt-teleop](../../README.md).
This is the single integration crate that every domain adapter depends on.

## Modules

- `dti` — the `DomainTeleopInterface` trait that a domain adapter implements,
  plus `OperatorId`, `Result`, and `DomainError`.
- `wire` — rkyv-derived sensor and state structs exchanged with tpt-teleop
  (`SensorFeed`, `DomainState`, `Pose6DOF`, …).
- `lifecycle` — the autonomy ↔ teleop handover state machine and its mapping
  onto the core `Mode`.
- `session` — `DomainSession`, the driver that enforces the lifecycle.

## Usage

This crate is part of the tpt-teleop workspace and is not published to
crates.io. Depend on it by path or git:

```toml
[dependencies]
tpt-t-domain-bridge = { path = "crates/tpt-t-domain-bridge" }
```

A domain adapter implements `DomainTeleopInterface` and hands it to a
`DomainSession`. Run the session tests with
`cargo test -p tpt-t-domain-bridge`.

## Changelog

See [CHANGELOG.md](CHANGELOG.md).

## License

Dual-licensed under MIT OR Apache-2.0, at your option. See the workspace
`LICENSE-MIT` and `LICENSE-APACHE`.
