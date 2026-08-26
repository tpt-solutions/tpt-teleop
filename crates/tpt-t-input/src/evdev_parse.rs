//! Pure Linux-evdev wire-format parser.
//!
//! Kept free of `libc` calls and platform gates so the record layout,
//! normalization math, and event application are unit-testable on every
//! host (the syscalls live in [`crate::linux_evdev`]).

use crate::report::{slot, ControllerReport};

/// event types
pub const EV_SYN: u16 = 0x00;
pub const EV_KEY: u16 = 0x01;
pub const EV_ABS: u16 = 0x03;

/// abs axis codes we consume
pub const ABS_X: u16 = 0x00;
pub const ABS_Y: u16 = 0x01;
pub const ABS_Z: u16 = 0x02;
pub const ABS_RX: u16 = 0x03;
pub const ABS_RY: u16 = 0x04;
pub const ABS_RZ: u16 = 0x05;
pub const ABS_THROTTLE: u16 = 0x06;
pub const ABS_RUDDER: u16 = 0x07;
/// First hat-switch code (reports as two relative-style axes via value −1/0/1).
pub const ABS_HAT0X: u16 = 0x10;

/// Size of one `struct input_event` on 64-bit Linux (timeval 16 B + 8 B payload).
pub const EVENT_SIZE: usize = 24;

/// One decoded `struct input_event`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EvdevEvent {
    /// Event timestamp seconds (from kernel, not wall clock).
    pub tv_sec: i64,
    /// Event timestamp microseconds.
    pub tv_usec: i64,
    /// Event type ([`EV_SYN`], [`EV_KEY`], [`EV_ABS`], …).
    pub kind: u16,
    /// Event code (axis or button code).
    pub code: u16,
    /// Event value (axis position or key press state).
    pub value: i32,
}

/// Parses a little-endian `input_event` record from `buf` at `off`.
/// Returns `None` when fewer than [`EVENT_SIZE`] bytes remain.
pub fn decode_event(buf: &[u8], off: usize) -> Option<EvdevEvent> {
    if buf.len() < off + EVENT_SIZE {
        return None;
    }
    let rd = |o: usize| -> u64 {
        let mut v = 0u64;
        for k in 0..8 {
            v |= (buf[off + o + k] as u64) << (8 * k);
        }
        v
    };
    let tv_sec = rd(0) as i64;
    let tv_usec = rd(8) as i64;
    let lo = u16::from_le_bytes([buf[off + 16], buf[off + 17]]);
    let code = u16::from_le_bytes([buf[off + 18], buf[off + 19]]);
    let value = i32::from_le_bytes([
        buf[off + 20],
        buf[off + 21],
        buf[off + 22],
        buf[off + 23],
    ]);
    Some(EvdevEvent { tv_sec, tv_usec, kind: lo, code, value })
}

/// Accumulates decoded events into a report under per-axis calibration
/// (`min`/`max` pairs captured at device-open time via `EVIOCGABS`).
#[derive(Debug, Clone)]
pub struct EvdevAccumulator {
    /// Calibration table indexed by axis slot.
    pub calib: [(i32, i32); 8],
    /// Buttons bitfield.
    pub buttons: u32,
    /// Normalized axes.
    pub axes: [f32; 8],
    /// Last event timestamp split (sec, usec).
    pub last_time: (i64, i64),
}

impl Default for EvdevAccumulator {
    fn default() -> Self {
        // Sane defaults for gamepads: sticks ±1023 (some report ±255),
        // triggers/hats 0..=1023. Real ranges overwrite at open time.
        Self {
            calib: [
                (-1023, 1023),
                (-1023, 1023),
                (-1023, 1023),
                (-1023, 1023),
                (-1023, 1023),
                (-1023, 1023),
                (0, 1023),
                (0, 1023),
            ],
            buttons: 0,
            axes: [0.0; 8],
            last_time: (0, 0),
        }
    }
}

impl EvdevAccumulator {
    /// Applies one raw absolute-axis sample into its normalized slot.
    pub fn push_abs(&mut self, code: u16, value: i32) -> bool {
        let (slot_idx, invert) = match code {
            ABS_X => (slot::ROLL, false),
            ABS_Y => (slot::PITCH, true), // stick Y reports forward-negative
            ABS_RX => (slot::YAW, false),
            ABS_Z | ABS_RZ if code == ABS_RZ => (slot::THROTTLE, false),
            ABS_Z => (slot::SPARE0, false),
            ABS_THROTTLE => (slot::THROTTLE, false),
            ABS_RUDDER => (slot::YAW, false),
            ABS_HAT0X => (slot::LAT_X, false),
            _ => return false,
        };
        let (min, max) = self.calib[slot_idx.min(7)];
        let span = (max - min).max(1);
        let norm = ((value - min) as f32 / span as f32) * 2.0 - 1.0;
        self.axes[slot_idx] = if invert { -norm.clamp(-1.0, 1.0) } else { norm.clamp(-1.0, 1.0) };
        true
    }

    /// Applies one key/button press-release.
    pub fn push_key(&mut self, code: u16, down: bool) -> bool {
        let bit = (code % 32) as u32;
        if down {
            self.buttons |= 1 << bit;
        } else {
            self.buttons &= !(1 << bit);
        }
        true
    }

    /// Decodes + applies one parsed event. SYN only updates the timestamp.
    pub fn push(&mut self, ev: &EvdevEvent) -> bool {
        self.last_time = (ev.tv_sec, ev.tv_usec);
        match ev.kind {
            EV_ABS => self.push_abs(ev.code, ev.value),
            EV_KEY => self.push_key(ev.code, ev.value != 0),
            _ => false,
        }
    }

    /// Copies current accumulated state into `out` with timestamp from the
    /// last observed event.
    pub fn snapshot(&self, out: &mut ControllerReport) {
        out.axes = self.axes;
        out.buttons = self.buttons;
        out.timestamp_ns =
            (self.last_time.0 as u64).wrapping_mul(1_000_000_000)
                .wrapping_add(self.last_time.1 as u64);
    }
}
