//! Motor interface.

use crate::types::{MotorCommand, MotorTelemetry};

/// One motor channel.
///
/// Commands flow in via [`apply`](Motor::apply); read-back flows out through
/// [`read`](Motor::read) into a caller-provided record — no allocation on
/// either path. Real backends will wrap SocketCAN/CAN-open nodes or PWM
/// memory-mapped registers; the simulator provides `sim::drone`.
pub trait Motor: Send {
    /// Latches the latest command. Implementations apply it at their own
    /// cadence (next control tick), matching real ESC behavior.
    fn apply(&mut self, cmd: &MotorCommand);

    /// Reads current state into `out`, stamped at read time.
    fn read(&mut self, out: &mut MotorTelemetry);
}
