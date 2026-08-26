//! Cross-platform CAN backend stub (spec §5.2 "cross-platform CAN backend
//! (macOS/Windows)").
//!
//! macOS/Windows lack SocketCAN; their CAN stacks (vendor APIs, PEAK,
//! Crossbow, etc.) bind at hardware bring-up. Until then this fails loudly so
//! a session never silently runs without a bus.

use crate::can::CanBus;
use crate::types::{CanFrame, HalError};

/// Placeholder CAN source; construction always fails with
/// [`HalError::Unsupported`].
#[derive(Debug, Default)]
pub struct StubCan {
    _private: (),
}

impl StubCan {
    /// Always returns [`HalError::Unsupported`]; see module docs.
    pub fn open(_iface: &str) -> Result<Self, HalError> {
        Err(HalError::Unsupported(
            "cross-platform CAN deferred to hardware bring-up",
        ))
    }
}

impl CanBus for StubCan {
    fn send(&mut self, _frame: &CanFrame) -> Result<(), HalError> {
        Err(HalError::Unsupported("cross-platform CAN deferred"))
    }
    fn recv(&mut self, _out: &mut CanFrame) -> bool {
        false
    }
}
