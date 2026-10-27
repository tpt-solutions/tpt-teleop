# tpt-t-cli

Command-line tool for [tpt-teleop](../../README.md): project scaffolding,
cargo-deny MIT-chain config generation, CPU core-pinning profiles, FDR replay,
a simulator readout, an MCP fleet-dispatch console, and an environment check.

## Commands

```text
tpt-t-cli scaffold <NAME> [--path <DIR>]   Create a new robot project
tpt-t-cli deny     [--out <FILE>]          Generate the cargo-deny MIT-chain config
tpt-t-cli profile  [--cores <N>] [--out <FILE>]  Generate a CPU core-pinning profile
tpt-t-cli replay   ...                     Replay a recorded FDR file
tpt-t-cli sim      ...                     Live terminal readout of the simulator and safety loop
tpt-t-cli console  ...                     MCP fleet-dispatch console
tpt-t-cli doctor                           Check the toolchain and environment
```

Run `tpt-t-cli --help` for the full option list.

## Installation

This crate is part of the tpt-teleop workspace and is not published to
crates.io. From the repository root:

```bash
cargo run -p tpt-t-cli -- doctor
```

## Changelog

See [CHANGELOG.md](CHANGELOG.md).

## License

Dual-licensed under MIT OR Apache-2.0, at your option. See the workspace
`LICENSE-MIT` and `LICENSE-APACHE`.
