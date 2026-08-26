//! Source abstraction: one opened raw-HID device.

use crate::report::ControllerReport;

/// Failures surfaced while opening or reading devices.
#[derive(Debug)]
pub enum InputError {
    /// OS open/read call failed.
    Os(String),
    /// Backend does not exist on this platform.
    Unsupported(&'static str),
    /// Caller supplied a malformed device path.
    BadPath(&'static str),
}

impl core::fmt::Display for InputError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            InputError::Os(s) => write!(f, "input os error: {s}"),
            InputError::Unsupported(s) => write!(f, "unsupported on this platform: {s}"),
            InputError::BadPath(s) => write!(f, "bad device path: {s}"),
        }
    }
}

impl std::error::Error for InputError {}

/// One opened raw input device, polled synchronously.
///
/// `poll` never blocks forever and never allocates: implementations use
/// non-blocking descriptors (Linux) or timed overlapped reads (Windows).
/// A `false` return means "no fresh report this tick"; callers simply poll
/// again next loop iteration.
pub trait RawInputSource: Send {
    /// Reads the newest pending report into `out`; `false` if none pending.
    fn poll(&mut self, out: &mut ControllerReport) -> bool;

    /// Static identity captured at open time.
    fn info(&self) -> &crate::report::DeviceInfo;

    /// Releases and reopens the underlying handle (error recovery path).
    fn reopen(&mut self) -> Result<(), InputError>;
}
