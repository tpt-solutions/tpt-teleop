# tpt-t-ring

Wait-free SPSC ring buffers for [tpt-teleop](../../README.md), with zero-copy
pointer passing and byte-to-struct casting.

- **`SpscRing`** — a heap-backed, bounded, wait-free SPSC queue. Neither end
  blocks, spins, or allocates. Capacity rounds up to a power of two, so indexing
  is a single AND.
- **`shared::SharedSpsc`** — the same algorithm over caller-provided memory,
  such as shared memory or mmapped regions, for cross-process IPC.
- **`ptr`** — pointer-passing rings that hand pooled buffers between threads
  without copying payload bytes.
- **`cast`** — zero-copy byte slice ↔ struct casting for `#[repr(C)]` plain-old-data types.
- **`layout`** — deterministic shared-memory layout math.

## Usage

This crate is part of the tpt-teleop workspace and is not published to
crates.io. Depend on it by path or git:

```toml
[dependencies]
tpt-t-ring = { path = "crates/tpt-t-ring" }
```

Run the ring benchmark with `cargo bench -p tpt-t-ring`.

## Changelog

See [CHANGELOG.md](CHANGELOG.md).

## License

Dual-licensed under MIT OR Apache-2.0, at your option. See the workspace
`LICENSE-MIT` and `LICENSE-APACHE`.
