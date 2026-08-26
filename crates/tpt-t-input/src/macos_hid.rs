//! macOS backend stub.
//!
//! Apple exposes gamepads/HID exclusively through IOKit's `IOHIDManager`,
//! whose FFI surface is far outside what `libc` covers. Binding it properly
//! (via hand-rolled IOKit externs or a MIT/Apache dual-licensed binding
//! crate that passes the deny gate) is scheduled alongside real-hardware
//! bring-up; until then this backend reports [`InputError::Unsupported`] so
//! callers fail loudly instead of silently missing devices.

use crate::report::{ControllerReport, DeviceInfo};
use crate::source::{InputError, RawInputSource};

/// Placeholder macOS source; construction always fails with
/// [`InputError::Unsupported`].
#[derive(Debug, Default)]
pub struct MacHidSource;

impl MacHidSource {
    /// Always returns [`InputError::Unsupported`]; see module docs.
    pub fn open(_path: &str) -> Result<Self, InputError> {
        Err(InputError::Unsupported("macOS IOKit HID binding deferred"))
    }
}

impl RawInputSource for MacHidSource {
    fn poll(&mut self, _out: &mut ControllerReport) -> bool {
        false
    }

    fn info(&self) -> &DeviceInfo {
        unreachable!("MacHidSource can never be constructed");
    }

    fn reopen(&mut self) -> Result<(), InputError> {
        Err(InputError::Unsupported("macOS IOKit HID binding deferred"))
    }
}
