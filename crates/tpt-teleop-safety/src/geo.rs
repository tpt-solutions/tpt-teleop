//! Geofencing: cylindrical keep-in volume with soft-warning and hard-breach
//! verdicts, plus command-space clamping that steers back inside.

use tpt_teleop_core::ser::ControlCommand;
use tpt_teleop_hal::Pose6D;

/// Axis indices inside [`ControlCommand::axes`](ControlCommand).
pub mod axis {
    /// Roll (normalized −1..1).
    pub const ROLL: usize = 0;
    /// Pitch (normalized −1..1).
    pub const PITCH: usize = 1;
    /// Yaw rate (normalized −1..1).
    pub const YAW: usize = 2;
    /// Throttle (normalized 0..1; 0.5 ≈ hover-neutral).
    pub const THROTTLE: usize = 3;
    /// World-frame lateral X velocity command (normalized).
    pub const LAT_X: usize = 4;
    /// World-frame lateral Y velocity command (normalized).
    pub const LAT_Y: usize = 5;
}

/// Hover-neutral throttle used when the fence suppresses climb/descent.
pub const THROTTLE_NEUTRAL: f32 = 0.5;

/// Cylindrical keep-in geofence (flat-earth approximation).
#[derive(Debug, Clone, Copy)]
pub struct GeoFence {
    /// Fence center, east offset (m).
    pub center_east_m: f64,
    /// Fence center, north offset (m).
    pub center_north_m: f64,
    /// Allowed horizontal radius (m).
    pub radius_m: f64,
    /// Minimum altitude (m).
    pub min_alt_m: f64,
    /// Maximum altitude (m).
    pub max_alt_m: f64,
}

impl Default for GeoFence {
    fn default() -> Self {
        Self {
            center_east_m: 0.0,
            center_north_m: 0.0,
            radius_m: 500.0,
            min_alt_m: 0.0,
            max_alt_m: 120.0,
        }
    }
}

/// Result of evaluating one pose against the fence.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FenceVerdict {
    /// Comfortably inside all bounds.
    Inside,
    /// Within the warning band (outer 5 % of any bound): commands get
    /// progressively attenuated but flight continues.
    SoftWarn,
    /// Past a bound: outward motion is zeroed and the loop policy is
    /// expected to raise an emergency stop.
    HardBreach,
}

impl GeoFence {
    /// Horizontal distance from fence center (m).
    #[inline]
    pub fn horizontal_dist(&self, pose: &Pose6D) -> f64 {
        let dx = pose.x - self.center_east_m;
        let dy = pose.y - self.center_north_m;
        dx.hypot(dy)
    }

    /// Classifies a pose. The soft band is the outer 5 % of every bound;
    /// anything past it is a hard breach.
    pub fn evaluate(&self, pose: &Pose6D) -> FenceVerdict {
        let d = self.horizontal_dist(pose);
        let soft_r = self.radius_m * 0.95;
        let soft_hi = self.max_alt_m * 0.95;

        if d > self.radius_m || pose.z > self.max_alt_m || pose.z < self.min_alt_m {
            return FenceVerdict::HardBreach;
        }
        if d > soft_r || pose.z > soft_hi {
            return FenceVerdict::SoftWarn;
        }
        FenceVerdict::Inside
    }
}

impl GeoFence {
    /// Mutates `cmd` in place, suppressing components that would push the
    /// vehicle further outside after a breach:
    ///
    /// * horizontal: removes the outward component of the lateral and
    ///   attitude axes (projection onto the inward half-plane),
    /// * vertical: pins throttle to hover-neutral past the ceiling.
    ///
    /// `verdict` gates aggressiveness: `SoftWarn` scales attenuation with
    /// breach depth, `HardBreach` zeroes outward motion entirely.
    pub fn clamp_command(&self, pose: &Pose6D, verdict: FenceVerdict, cmd: &mut ControlCommand) {
        let dx = pose.x - self.center_east_m;
        let dy = pose.y - self.center_north_m;
        let dist = dx.hypot(dy);
        if dist > 1e-9 {
            let (ux, uy) = (dx / dist, dy / dist);
            let severity = if verdict == FenceVerdict::HardBreach {
                1.0f64
            } else {
                ((dist - self.radius_m * 0.95) / (self.radius_m * 0.05)).clamp(0.0, 1.0)
            };
            if severity > 0.0 {
                for (axis_idx, u) in [(axis::LAT_X, ux), (axis::LAT_Y, uy)] {
                    let outward = cmd.axes[axis_idx] as f64 * u;
                    if outward > 0.0 {
                        cmd.axes[axis_idx] -= (outward * severity * u) as f32;
                    }
                }
                // Attitude commands lean the vehicle — same outward logic.
                for (axis_idx, u) in [(axis::ROLL, ux), (axis::PITCH, uy)] {
                    let outward = cmd.axes[axis_idx] as f64 * u;
                    if outward > 0.0 {
                        cmd.axes[axis_idx] -= (outward * severity * u) as f32;
                    }
                }
            }
        }

        // Vertical suppression past the ceiling.
        if pose.z >= self.max_alt_m * 0.95 && cmd.axes[axis::THROTTLE] > THROTTLE_NEUTRAL {
            cmd.axes[axis::THROTTLE] = THROTTLE_NEUTRAL;
        }
        if pose.z <= self.min_alt_m && cmd.axes[axis::THROTTLE] < THROTTLE_NEUTRAL {
            cmd.axes[axis::THROTTLE] = THROTTLE_NEUTRAL;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tpt_teleop_core::Mode;

    fn pose_at(x: f64, y: f64, z: f64) -> Pose6D {
        Pose6D { x, y, z, ..Pose6D::default() }
    }

    #[test]
    fn verdict_zones_are_ordered() {
        let fence = GeoFence::default(); // r=500, z∈[0,120]
        assert_eq!(fence.evaluate(&pose_at(0.0, 0.0, 50.0)), FenceVerdict::Inside);
        assert_eq!(fence.evaluate(&pose_at(480.0, 0.0, 50.0)), FenceVerdict::SoftWarn);
        assert_eq!(fence.evaluate(&pose_at(501.0, 0.0, 50.0)), FenceVerdict::HardBreach);
        assert_eq!(fence.evaluate(&pose_at(0.0, 0.0, 119.0)), FenceVerdict::SoftWarn);
        assert_eq!(fence.evaluate(&pose_at(0.0, 0.0, 121.0)), FenceVerdict::HardBreach);
    }

    #[test]
    fn clamp_removes_outward_lateral_component_on_hard_breach() {
        let fence = GeoFence::default();
        let pose = pose_at(600.0, 0.0, 50.0); // due east of center
        let mut cmd = ControlCommand::zeroed(Mode::Assist);
        cmd.axes[axis::LAT_X] = 1.0; // pushing east = further out

        fence.clamp_command(&pose, FenceVerdict::HardBreach, &mut cmd);
        assert!(cmd.axes[axis::LAT_X] <= 1e-6, "outward push must vanish");

        // Inward / crosswind components are untouched.
        let mut keep = ControlCommand::zeroed(Mode::Assist);
        keep.axes[axis::LAT_X] = -1.0;
        keep.axes[axis::LAT_Y] = 0.5;
        fence.clamp_command(&pose, FenceVerdict::HardBreach, &mut keep);
        assert_eq!(keep.axes[axis::LAT_X], -1.0);
        assert_eq!(keep.axes[axis::LAT_Y], 0.5);
    }

    #[test]
    fn soft_warning_attenuates_partially_not_fully() {
        let fence = GeoFence { radius_m: 100.0, ..GeoFence::default() };
        let pose = pose_at(98.0, 0.0, 50.0); // 60 % into the soft band
        let mut cmd = ControlCommand::zeroed(Mode::Assist);
        cmd.axes[axis::LAT_X] = 1.0;

        fence.clamp_command(&pose, FenceVerdict::SoftWarn, &mut cmd);
        let v = cmd.axes[axis::LAT_X];
        assert!(v > 0.2 && v < 0.9, "soft warn must partially attenuate, got {v}");
    }

    #[test]
    fn clamp_pins_throttle_past_ceiling() {
        let fence = GeoFence::default();
        let pose = pose_at(0.0, 0.0, 130.0);
        let mut cmd = ControlCommand::zeroed(Mode::Assist);
        cmd.axes[axis::THROTTLE] = 0.9;
        fence.clamp_command(&pose, FenceVerdict::HardBreach, &mut cmd);
        assert!((cmd.axes[axis::THROTTLE] - THROTTLE_NEUTRAL).abs() < 1e-6);
    }
}
