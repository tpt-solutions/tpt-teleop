# tpt-t-link

The link layer for [tpt-teleop](../../README.md): everything between the safety
loop's output ring and the wire.

- **UDP multiplexer** — control, telemetry, media, ICE, and mesh traffic share
  one port (default UDP 443) behind an 8-byte wire header plus a 4-byte
  channel/flags word. rkyv writes straight into the datagram buffer.
- **Service loop** — drives the mux from the platform event loop (epoll,
  kqueue, or IOCP) on one pinned network thread. No async runtime.
- **io_uring transmit** (Linux) — a zero-copy transmit path.
- **Secure channel** — end-to-end encrypted framing via `tpt-t-sec`.
- **Reliable transport** — in-house selective-repeat, plus backpressure
  signalling and mesh neighbor discovery.

## Usage

This crate is part of the tpt-teleop workspace and is not published to
crates.io. Depend on it by path or git:

```toml
[dependencies]
tpt-t-link = { path = "crates/tpt-t-link" }
```

Run the serialization benchmark with `cargo bench -p tpt-t-link`.

## Changelog

See [CHANGELOG.md](CHANGELOG.md).

## License

Dual-licensed under MIT OR Apache-2.0, at your option. See the workspace
`LICENSE-MIT` and `LICENSE-APACHE`.
