# tpt-t-input

Input subsystem for [tpt-teleop](../../README.md): raw HID controller polling,
a universal controller abstraction, OpenXR hand tracking, haptics routing, and
lock-free co-pilot arbitration. Commands flow zero-copy into the safety loop.

## Backends

| Platform | Status |
|----------|--------|
| Linux | Raw evdev over syscalls (live) |
| Windows | Raw HID with overlapped reads (live) |
| macOS | IOKit is a documented stub |
| OpenXR | Integration trait with a null source until a runtime ships |

## Usage

This crate is part of the tpt-teleop workspace and is not published to
crates.io. Depend on it by path or git:

```toml
[dependencies]
tpt-t-input = { path = "crates/tpt-t-input" }
```

Run the polling benchmark with `cargo bench -p tpt-t-input`.

## Changelog

See [CHANGELOG.md](CHANGELOG.md).

## License

Dual-licensed under MIT OR Apache-2.0, at your option. See the workspace
`LICENSE-MIT` and `LICENSE-APACHE`.
