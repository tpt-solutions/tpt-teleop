//! Input subsystem: raw HID polling, universal controller abstraction,
//! OpenXR hand-tracking surface, haptics routing, and lock-free co-pilot
//! arbitration: feeding zero-copy commands into the safety loop.
//!
//! Real-device backends: Linux evdev (raw syscalls) and Windows HID
//! (overlapped reads) are live; macOS IOKit is a documented stub; OpenXR
//! exposes its integration trait with a null source until a runtime ships.

pub mod ai_source;
pub mod copilot;
pub mod evdev_parse;
pub mod haptics;
#[cfg(target_os = "linux")]
pub mod linux_evdev;
#[cfg(target_os = "macos")]
pub mod macos_hid;
pub mod map;
pub mod pipeline;
pub mod report;
pub mod source;
#[cfg(windows)]
pub mod win_hid;
pub mod xr;

pub use ai_source::{AiCommandSource, CommandSource, Origin};
pub use copilot::{CoPilotHub, MAX_OPERATORS};
pub use haptics::{HapticEffect, HapticRouter, HapticSink, MockHaptics};
#[cfg(target_os = "linux")]
pub use linux_evdev::EvdevSource;
#[cfg(target_os = "macos")]
pub use macos_hid::MacHidSource;
pub use map::ControllerMap;
pub use pipeline::{CommandStage, InputStage};
pub use report::{ControllerReport, DeviceInfo, slot};
pub use source::{InputError, RawInputSource};
#[cfg(windows)]
pub use win_hid::WinHidSource;
pub use xr::{HandPose, HandTrackingSource, NullHandTracking};

/// Crate version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
