//! Sensor interfaces — typed, zero-copy reads into core wire structs.

use tpt_t_core::ser::{GpsSample, ImuSample, TelemetryPacket};

/// Inertial measurement unit source.
pub trait ImuSource: Send {
    /// Fills `out` with the freshest sample; `false` when no new sample
    /// since the previous call (callers keep their previous values).
    fn read(&mut self, out: &mut ImuSample) -> bool;
}

/// GNSS receiver source.
pub trait GpsSource: Send {
    /// See [`ImuSource::read`] for the contract.
    fn read(&mut self, out: &mut GpsSample) -> bool;
}

/// Generic multi-value telemetry source (battery, temperatures, …).
pub trait TelemetrySource: Send {
    /// See [`ImuSource::read`] for the contract.
    fn read(&mut self, out: &mut TelemetryPacket) -> bool;
}
