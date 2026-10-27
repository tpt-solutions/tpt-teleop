# tpt-t-hal

Hardware abstraction layer for [tpt-teleop](../../README.md): motors, sensors,
CAN bus, and cameras behind common traits.

## Backends

- **Mock / simulator** — a virtual CAN bus over SPSC rings, deterministic ESC
  models, a null camera, and a physics quadrotor (`sim::QuadDrone`) for
  end-to-end tests. Runs everywhere with no hardware.
- **SocketCAN** (Linux) — raw `AF_CAN` access over `libc` FFI.
- **Stub CAN** (other platforms) — fails with `HalError::Unsupported` until a
  vendor CAN stack is bound.
- **MAVLink 1.0/2.0** — a from-scratch parser with CRC-16/MCRF4XX and dialect
  `crc_extra` tables, decoding into rkyv structs.
- **MMIO** — `BufferMmio` for simulation and `LinuxMmio` over `/dev/mem`.

A backend swap test (`tests/backend_swap.rs`) runs one generic `CanBus`
harness against the mock, the stub, and SocketCAN.

## Usage

This crate is part of the tpt-teleop workspace and is not published to
crates.io. Depend on it by path or git:

```toml
[dependencies]
tpt-t-hal = { path = "crates/tpt-t-hal" }
```

Run the simulator benchmark with `cargo bench -p tpt-t-hal`.

## Changelog

See [CHANGELOG.md](CHANGELOG.md).

## License

Dual-licensed under MIT OR Apache-2.0, at your option. See the workspace
`LICENSE-MIT` and `LICENSE-APACHE`.
