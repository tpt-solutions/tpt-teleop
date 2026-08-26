# tpt-teleop Architecture

Version 1.0.0 · August 2026 · TPT Solutions

This document maps the workspace crates and the zero-copy data flow described
in `spec.txt` §6.

## 1. Crate Map

```
                       ┌────────────────────┐
                       │   tpt-t-cli   │  scaffolding / deny config /
                       └─────────┬──────────┘  core-pinning profiles
                                 │ uses
        ┌────────────────────────┼────────────────────────┐
        │                 tpt-t-core                  │
        │     state machine · message bus · event loop     │
        │     thread pinning · rkyv types · buffer pool    │
        └──┬──────┬──────────┬───────────┬──────────┬─────┘
           │      │          │           │          │
    tpt-t-input│  tpt-t-media│  tpt-t-cloud
           │      │          │           │          │
    tpt-t-safety   tpt-t-link  tpt-t-sec
           │      │                      │
           └──────┴──── tpt-t-hal ──┘
                          │
                  tpt-t-analytics
```

Foundation layers (no upward deps):

* **tpt-t-ring** — wait-free SPSC ring buffers over shared memory layouts;
  zero-copy pointer passing and byte↔struct casting utilities. Everything
  communicates through these; nothing allocates or locks in the hot path.
* **tpt-t-core** — central state machine (Auto/Assist/FullTeleop/
  EmergencyStop), custom lock-free MPMC queue + fan-out message bus, the
  platform event-loop abstraction (io_uring / kqueue / IOCP), CPU core-pinning
  helpers and profile configs, and the rkyv wire types (`ControlCommand`,
  telemetry packets) with pre-allocated serialization pools.

Subsystems (depend on foundation only):

* **tpt-t-input** polls raw HID/evdev reports and casts them zero-copy
  into `ControlCommand`s.
* **tpt-t-safety** runs on a dedicated RT thread, pops commands from a
  ring, applies geofence/kinematic limits in place, pushes to the output ring.
* **tpt-t-link** multiplexes control/telemetry/WebRTC ICE over one UDP
  port, serializing with rkyv directly into pre-allocated packet buffers.
* **tpt-t-media** ingests camera frames into slab-allocated frame pools
  and hardware-encodes with telemetry burn-in.
* **tpt-t-hal** abstracts motors/sensors/CAN/cameras behind one trait set;
  ships a rapier-based physics simulator backend plus real SocketCAN/MAVLink
  backends.
* **tpt-t-cloud**, **tpt-t-sec**, **tpt-t-analytics** provide
  fleet HTTP/3 + SFU, zero-trust E2EE, and direct-I/O FDR logging with AI
  export respectively.


## 2. Threading Model — Thread Per Core

There is no global async runtime. Each latency-critical role owns dedicated
CPU cores, pinned at startup:

| Role | Cores (example profile) | Loop driver |
|------|--------------------------|-------------|
| Video encode | 0 | media encoder thread |
| Control loop | 1 | safety RT thread (SCHED_FIFO / equivalent) |
| Network I/O | 2 | platform event loop |
| Input | 3 | HID poller |
| Storage/FDR | 4 | analytics writer |

Profiles live in a plain-text core-pinning config parsed by
`tpt-t-core::profile` (see that module's docs for the format).

## 3. Event Loops

One abstraction (`tpt-t-core::eventloop::EventLoop`), three backends:

* Linux — raw `io_uring` submission/completion rings (`io-uring` crate).
* macOS/BSD — `kqueue`/`kevent` via `libc`.
* Windows — I/O completion ports via `windows-sys`.

Backends are compiled per-target and selected automatically; user code only
handles `Token` readiness callbacks.

## 4. The TPT Data Flow (Zero-Copy Path)

From `spec.txt` §6:

```
 Ingest     tpt-t-input reads a raw HID report from the OS.
            │
 Normalize  raw bytes cast (zero-copy) into a ControlCommand struct.
            │
 Route      struct pushed into a tpt-t-ring SPSC queue.
            │
 Safety     safety loop pops, checks geofences, mutates in place,
            │       pushes to the next ring.
            │
 Serialize  tpt-t-link rkyv-serializes directly into a
            │       pre-allocated UDP packet buffer.
            ▼
 Transmit   buffer handed to io_uring for zero-copy kernel send.

 Total allocations in this path: zero.
 Total mutex locks in this path:  zero.
```

## 5. Serialization Conventions

* All wire structs are `#[repr(C)]`, plain-old-data, and derive
  `rkyv::{Archive, Serialize, Deserialize}`.
* Archived buffers are aligned to 64 bytes (one cache line).
* Hot-path ingress may bypass rkyv entirely using
  `tpt-t-ring::cast` (byte slice ↔ struct casting) when both ends share
  endianness/layout — e.g. shared-memory IPC on the same machine.
* Every frame carries a magic/version header for forward compatibility.

## 5.1. Flight Data Recorder & AI Export (tpt-t-analytics)

FDR logging must never stall the control loop, so the hot path never touches
disk. Each subsystem publishes a fixed-size [`FdrEntry`](crates/tpt-t-analytics/src/record.rs)
(a `repr(C)` header + inline payload carrying the rkyv wire bytes of a
`ControlCommand` / telemetry sample) through a wait-free
[`tpt-t-ring::SpscRing`](crates/tpt-t-ring/src/spsc.rs). A full ring returns
`RecordError::Full` immediately — the producer sheds the record rather than
block. A dedicated storage thread (role "Storage/FDR" in the core-pinning
profile) drains the ring and writes the bytes to a
[`DirectFile`](crates/tpt-t-analytics/src/direct_io.rs) that bypasses the page
cache via OS direct I/O:

* Linux — `O_DIRECT` (falls back to buffered if the filesystem rejects it).
* Windows — `CreateFileA` with `FILE_FLAG_NO_BUFFERING | FILE_FLAG_WRITE_THROUGH`
  (note: `FILE_GENERIC_WRITE` is avoided because it implies `FILE_APPEND_DATA`,
  which is incompatible with unbuffered I/O).
* macOS — `F_NOCACHE` via `fcntl`.

All three stage bytes into a 4096-byte-aligned buffer and flush in
sector-multiple blocks. An end-of-stream marker lets offline readers stop
before the zero-padding direct I/O appends to the final sector.

The AI export path is fully offline: an FDR file (or in-memory entries) is
parsed back into wire structs and turned into feature/label tensors
serialized as NumPy `.npy` — the interchange format both PyTorch
(`numpy.load` → `torch.from_numpy`) and JAX (`numpy.load` → `jnp.asarray`)
consume natively, so a single writer serves both (spec §5.8).

## 6. Licensing & Dependency Policy

Enforced by `deny.toml`: allowed licenses are MIT, BSD-2/3-Clause, ISC, Zlib,
MPL-2.0. Dual MIT/Apache-2.0 crates are clarified to MIT resolution; strictly
Apache-2.0-only crates are banned (see README).
