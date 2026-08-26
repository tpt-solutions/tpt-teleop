//! Predictive collision avoidance via kinematic limit enforcement.
//!
//! Two layers protect against commanded-motion collisions:
//!
//! 1. **Absolute clamps** — tilt / yaw-rate / lateral / throttle bounds.
//! 2. **Slew-rate limiting** — per-tick max delta toward the previous
//!    accepted command, so even a hostile step input produces a bounded
//!    acceleration profile instead of a discontinuity.
//!
//! "Predictive" comes from pairing this with the geofence projection:
//! commands projecting outward within the warning band are attenuated before
//! they can integrate into a breach (see [`crate::geo`]).

use tpt_teleop_core::ser::ControlCommand;

/// Axis indices (mirror of `crate::geo::axis`, kept local for speed).
const ROLL: usize = 0;
const PITCH: usize = 1;
const YAW: usize = 2;
const THROTTLE: usize = 3;
const LAT_X: usize = 4;
const LAT_Y: usize = 5;

/// Kinematic envelope applied to every command.
#[derive(Debug, Clone, Copy)]
pub struct KinematicLimits {
    /// |roll|, |pitch| clamp (normalized; ~20° tilt at 0.35).
    pub max_tilt_norm: f32,
    /// |yaw rate| clamp (normalized).
    pub max_yaw_norm: f32,
    /// Lateral velocity command clamp (normalized).
    pub max_lateral_norm: f32,
    /// Max per-tick change on any rotational/lateral axis (slew limit →
    /// bounded acceleration).
    pub max_axis_step: f32,
    /// Max per-tick throttle change (climb authority bound).
    pub max_throttle_step: f32,
}

impl Default for KinematicLimits {
    fn default() -> Self {
        // Tuned for a 200 Hz safety loop.
        Self {
            max_tilt_norm: 0.35,
            max_yaw_norm: 0.5,
            max_lateral_norm: 0.8,
            max_axis_step: 0.02,
            max_throttle_step: 0.01,
        }
    }
}

impl KinematicLimits {
    /// Mutates `cmd` in place: clamps absolutes, then slews each limited axis
    /// toward its new value from `prev` (the last command accepted by the
    /// loop). Returns `true` when any component was modified.
    pub fn apply(&self, prev: &ControlCommand, cmd: &mut ControlCommand) -> bool {
        let mut touched = false;

        for ax in [ROLL, PITCH] {
            touched |= self.clamp_abs(cmd, ax, self.max_tilt_norm);
        }
        touched |= self.clamp_abs(cmd, YAW, self.max_yaw_norm);
        for ax in [LAT_X, LAT_Y] {
            touched |= self.clamp_abs(cmd, ax, self.max_lateral_norm);
        }
        cmd.axes[THROTTLE] = cmd.axes[THROTTLE].clamp(0.0, 1.0);

        for ax in [ROLL, PITCH, YAW, LAT_X, LAT_Y] {
            touched |= self.slew(prev, cmd, ax, self.max_axis_step);
        }
        touched |= self.slew(prev, cmd, THROTTLE, self.max_throttle_step);
        touched
    }

    #[inline]
    fn clamp_abs(&self, cmd: &mut ControlCommand, ax: usize, limit: f32) -> bool {
        let before = cmd.axes[ax];
        cmd.axes[ax] = before.clamp(-limit, limit);
        before != cmd.axes[ax]
    }

    #[inline]
    fn slew(
        &self,
        prev: &ControlCommand,
        cmd: &mut ControlCommand,
        ax: usize,
        max_step: f32,
    ) -> bool {
        let target = cmd.axes[ax];
        let last = prev.axes[ax];
        let delta = (target - last).clamp(-max_step, max_step);
        let limited = last + delta;
        let changed = limited != target;
        cmd.axes[ax] = limited;
        changed
    }
}

/// Emergency override: overwrites `cmd` in place with the latched safe state
/// — all axes neutral, mode forced to the E-stop discriminant, flag bit 0
/// raised. This is the intercept path the RT loop takes whenever the E-stop
/// latch is set; it is unconditional and cannot be overridden upstream.
pub fn write_emergency_stop(seq: u64, timestamp_ns: u64, cmd: &mut ControlCommand) {
    *cmd = ControlCommand::zeroed(tpt_teleop_core::Mode::EmergencyStop);
    cmd.seq = seq;
    cmd.timestamp_ns = timestamp_ns;
    cmd.flags |= 0b0000_0001; // ESTOP flag bit
}

#[cfg(test)]
mod tests {
    use super::*;
    use tpt_teleop_core::Mode;

    #[test]
    fn absolute_clamps_engage() {
        let lim = KinematicLimits::default();
        let prev = ControlCommand::zeroed(Mode::FullTeleop);
        let mut cmd = ControlCommand::zeroed(Mode::FullTeleop);
        cmd.axes[ROLL] = 5.0;
        cmd.axes[PITCH] = -5.0;
        cmd.axes[YAW] = 2.0;
        cmd.axes[LAT_X] = 9.0;
        assert!(lim.apply(&prev, &mut cmd));
        assert_eq!(cmd.axes[ROLL], lim.max_tilt_norm);
        assert_eq!(cmd.axes[PITCH], -lim.max_tilt_norm);
        assert_eq!(cmd.axes[YAW], lim.max_yaw_norm);
        assert_eq!(cmd.axes[LAT_X], lim.max_lateral_norm);
    }

    #[test]
    fn slew_limiting_bounds_step_input() {
        let lim = KinematicLimits::default();
        let mut prev = ControlCommand::zeroed(Mode::FullTeleop);
        prev.axes[LAT_X] = 0.0;
        let mut cmd = prev;
        cmd.axes[LAT_X] = 1.0; // hostile step

        lim.apply(&prev, &mut cmd);
        assert!((cmd.axes[LAT_X] - 0.02).abs() < 1e-6);

        let mut steps = 0;
        while (prev.axes[LAT_X] - cmd.axes[LAT_X]).abs() > 1e-6 && steps < 200 {
            prev.axes[LAT_X] = cmd.axes[LAT_X];
            lim.apply(&prev, &mut cmd);
            steps += 1;
        }
        assert!(steps <= 60, "should converge in ≤60 ticks, took {steps}");
    }

    #[test]
    fn emergency_stop_overwrites_everything() {
        let mut cmd = ControlCommand::zeroed(Mode::FullTeleop);
        cmd.axes[THROTTLE] = 1.0;
        cmd.axes[ROLL] = 0.9;
        write_emergency_stop(42, 123, &mut cmd);
        assert_eq!(cmd.mode(), Some(Mode::EmergencyStop));
        assert_eq!((cmd.seq, cmd.timestamp_ns), (42, 123));
        assert_eq!(cmd.axes[THROTTLE], 0.0);
        assert_eq!(cmd.axes[ROLL], 0.0);
        assert_eq!(cmd.flags & 1, 1, "ESTOP flag bit must be set");
    }
}
