# Quick Start — building a robot with `#[derive(tpt_t::Robot)]`

tpt-teleop ships a procedural macro, `tpt_t::Robot`, that turns a plain struct
of device fields into a fully wired, lock-free, core-pinned robot. This guide
covers the Phase 13 developer-experience surface (Phase 14 validates it
end-to-end via `tpt-t-macros/tests/robot.rs`).

## 1. Declare the robot

Tag each device field with `#[camera(..)]` or `#[motor(..)]`. The derive
generates, per field:

* a lock-free `tpt_t_ring::SpscRing<Element>` in a companion `<Robot>Channels`
  struct (returned by `channels()`),
* a `launch()` that moves each device into its own thread, pinned to its
  `CoreProfile` role when `thread_per_core = true`,
* `serialize_*`, `push_*`, `pop_*` zero-copy wrappers.

```rust
use tpt_t::Robot;
use tpt_t_core::robot::RobotDevice;
use tpt_t_core::profile::CoreProfile;
use rkyv::{Archive, Deserialize, Serialize};

#[derive(Archive, Serialize, Deserialize)]
#[repr(C)]
struct Frame { seq: u32 }

#[derive(Archive, Serialize, Deserialize)]
#[repr(C)]
struct Command { throttle: f32 }

struct Camera;
impl RobotDevice for Camera {
    fn run(self) { /* poll sensor, push frames onto the ring */ }
}

struct Motor;
impl RobotDevice for Motor {
    fn run(self) { /* pull commands off the ring, drive actuator */ }
}

#[derive(Robot)]
#[robot(thread_per_core = true)]
struct Bot {
    #[camera(id = 0, element = Frame, capacity = 256)]
    cam: Camera,
    #[motor(id = 1, element = Command, capacity = 256)]
    arm: Motor,
}
```

## 2. Build the channel bundle and move data

The `<Robot>Channels` struct carries one ring per device. Use the generated
`push_*`/`pop_*` helpers for the lock-free hand-off and `serialize_*` for the
zero-copy rkyv path straight into a pre-allocated buffer.

```rust
let bot = Bot { cam: Camera, arm: Motor };

// One ring per device, pre-allocated.
let ch = bot.channels();

// Zero-copy hand-off between threads (no mutex, no allocation).
Bot::push_cam(&ch, Frame { seq: 1 }).unwrap();
Bot::push_arm(&ch, Command { throttle: 0.5 }).unwrap();

assert_eq!(Bot::pop_cam(&ch).unwrap().seq, 1);

// Zero-copy serialize into an aligned wire buffer.
let mut buf = tpt_t_core::ser::AlignedBuf::new();
let n = Bot::serialize_arm(&Command { throttle: 0.8 }, &mut buf).unwrap();
assert!(n > 0);
```

## 3. Pin each device to its own core

`launch` consumes the robot and runs every device on a dedicated, core-pinned
thread (Linux `sched_setaffinity` / macOS affinity / Windows
`SetThreadAffinityMask`), driven by a `CoreProfile`:

```rust
let bot = Bot { cam: Camera, arm: Motor };
let profile = CoreProfile::parse("video = 0\ncontrol = 1\n").unwrap();
let handles = bot.launch(&profile).expect("launch");
for h in handles { h.join().unwrap(); }
```

`Bot::THREAD_PER_CORE` and `Bot::roles()` expose the resolved configuration so
it can be asserted at compile time / inspected at runtime.

## 4. The full data plane (Phase 14)

`launch` gets devices onto pinned threads; the data plane that connects them is
the `Ingest → Normalize → Route → Safety → Serialize → Transmit` pipeline,
verified end-to-end in the `tpt-t-integration` crate:

```text
HID report ──Ingest──▶ Normalize ──Route──▶ Safety ──▶ Serialize ──▶ Transmit
```

Run the integration suite and the zero-lock audit:

```bash
cargo test -p tpt-t-integration
bash tools/lock-audit.sh
```

## Project scaffolding (CLI)

The `tpt-t-cli` crate also generates new projects, deny configs, and
core-pinning profiles:

```bash
cargo run -p tpt-t-cli -- scaffold my-bot
cargo run -p tpt-t-cli -- deny
cargo run -p tpt-t-cli -- profile
```
