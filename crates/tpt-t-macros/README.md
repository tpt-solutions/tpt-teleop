# tpt-t-macros

Procedural macros for [tpt-teleop](../../README.md). The `tpt_t` crate exposes
`#[derive(Robot)]`, which turns a plain struct into a running robot.

```text
#[derive(tpt_t::Robot)]
struct MyRobot {
    #[camera(...)]   // one SpscRing per tagged field
    front: ...,
    #[motor(...)]
    left: ...,
}
```

See `cargo doc -p tpt-t-macros` and the `robot` test for a complete example.

The derive generates three things:

1. **Lock-free rings** — an `SpscRing` for each `#[camera]` and `#[motor]`
   field, returned from a companion `<Robot>Channels` struct via `channels()`.
2. **Thread pinning** — `launch(self, &CoreProfile)` runs each device on its
   own CPU-pinned thread.
3. **Serialization wrappers** — `serialize_*`, `push_*`, and `pop_*` per device,
   with no intermediate allocation.

## Usage

This crate is part of the tpt-teleop workspace and is not published to
crates.io. Depend on it by path or git:

```toml
[dependencies]
tpt-t-macros = { path = "crates/tpt-t-macros" }
```

The crate's library name is `tpt_t`. Run the codegen tests with
`cargo test -p tpt-t-macros`.

## Changelog

See [CHANGELOG.md](CHANGELOG.md).

## License

Dual-licensed under MIT OR Apache-2.0, at your option. See the workspace
`LICENSE-MIT` and `LICENSE-APACHE`.
