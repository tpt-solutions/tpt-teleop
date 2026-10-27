# tpt-t-media

The media and telemetry layer for [tpt-teleop](../../README.md): everything
between a camera and the encoder that feeds the link.

- **Slab pool** — one contiguous allocation of fixed-size blocks with O(1)
  alloc and free for video frames and sensor packets.
- **Capture** — `TestPatternCapture` is the zero-dependency simulator source.
  V4L2, DirectShow, and AVFoundation backends return `MediaError::Unsupported`
  until hardware bring-up.
- **Burn-in** — a telemetry HUD rasterized into the pixel buffer before encode,
  using a built-in 5×7 font.
- **Encoder** — the `VideoEncoder` trait, a software `NullEncoder` for sim and
  tests, NVENC/AMF stubs, and an `EncoderGovernor` that slews bitrate toward the
  link's backpressure signal.

## Usage

This crate is part of the tpt-teleop workspace and is not published to
crates.io. Depend on it by path or git:

```toml
[dependencies]
tpt-t-media = { path = "crates/tpt-t-media" }
```

Run the burn-in benchmark with `cargo bench -p tpt-t-media`.

## Changelog

See [CHANGELOG.md](CHANGELOG.md).

## License

Dual-licensed under MIT OR Apache-2.0, at your option. See the workspace
`LICENSE-MIT` and `LICENSE-APACHE`.
