//! Universal controller abstraction: semantic mapping onto reports.

use tpt_t_core::ser::ControlCommand;

use crate::report::{ControllerReport, slot};

/// Mapping from raw device axes/buttons to semantic command channels.
///
/// One struct per connected device; pure function application keeps the
/// hot path allocation-free and trivially testable against synthetic
/// reports.
#[derive(Debug, Clone, Copy)]
pub struct ControllerMap {
    /// Axis slot feeding roll (default [`slot::ROLL`]).
    pub roll_slot: usize,
    /// Axis slot feeding pitch (default [`slot::PITCH`]).
    pub pitch_slot: usize,
    /// Axis slot feeding yaw rate (default [`slot::YAW`]).
    pub yaw_slot: usize,
    /// Axis slot feeding throttle (default [`slot::THROTTLE`]).
    pub throttle_slot: usize,
    /// Negate roll after read.
    pub invert_roll: bool,
    /// Negate pitch after read.
    pub invert_pitch: bool,
    /// Negate yaw after read.
    pub invert_yaw: bool,
    /// Deadzone applied to every axis (normalized units, symmetric).
    pub deadzone: f32,
}

impl Default for ControllerMap {
    fn default() -> Self {
        Self {
            roll_slot: slot::ROLL,
            pitch_slot: slot::PITCH,
            yaw_slot: slot::YAW,
            throttle_slot: slot::THROTTLE,
            invert_roll: false,
            invert_pitch: false,
            invert_yaw: false,
            deadzone: 0.04,
        }
    }
}

/// Applies the symmetric deadzone: values inside collapse to zero, outside
/// are rescaled so the response starts smoothly at the zone edge.
#[inline]
fn dz(v: f32, zone: f32) -> f32 {
    if zone <= 0.0 || v.abs() <= zone {
        0.0
    } else {
        (v - v.signum() * zone) / (1.0 - zone)
    }
}

impl ControllerMap {
    /// Maps one report onto `out`, mutating it in place (spec §6 Normalize).
    /// Buttons copy through verbatim; mode is left untouched.
    pub fn apply(&self, report: &ControllerReport, out: &mut ControlCommand) {
        let sgn = |inv: bool| if inv { -1.0 } else { 1.0 };
        let g = |slot_idx: usize, inv: bool| -> f32 {
            let raw = report.axes.get(slot_idx).copied().unwrap_or(0.0);
            let shaped = dz(raw.clamp(-1.0, 1.0), self.deadzone);
            shaped * sgn(inv)
        };

        out.axes[axis::ROLL] = g(self.roll_slot, self.invert_roll);
        out.axes[axis::PITCH] = g(self.pitch_slot, self.invert_pitch);
        out.axes[axis::YAW] = g(self.yaw_slot, self.invert_yaw);

        // Throttle: remap [-1,1] device space to [0,1] command space.
        let thr = report
            .axes
            .get(self.throttle_slot)
            .copied()
            .unwrap_or(0.0)
            .clamp(-1.0, 1.0);
        out.axes[axis::THROTTLE] = (thr * 0.5 + 0.5).clamp(0.0, 1.0);
    }
}

/// Safety-side axis indices (kept in sync with `tpt-t-safety::geo::axis`;
/// duplicated locally to avoid a runtime dependency inversion).
pub mod axis {
    /// Roll index.
    pub const ROLL: usize = 0;
    /// Pitch index.
    pub const PITCH: usize = 1;
    /// Yaw index.
    pub const YAW: usize = 2;
    /// Throttle index.
    pub const THROTTLE: usize = 3;
}

#[cfg(test)]
mod tests {
    use super::*;
    use tpt_t_core::Mode;

    fn report(axes: [f32; 8]) -> ControllerReport {
        ControllerReport {
            seq: 1,
            buttons: 0,
            axes,
            timestamp_ns: 42,
        }
    }

    #[test]
    fn default_map_routes_semantic_slots() {
        let m = ControllerMap::default();
        let mut out = ControlCommand::zeroed(Mode::FullTeleop);
        let rep = report([0.5, -0.25, 1.0, -1.0, 0.0, 0.0, 0.0, 0.0]);
        m.apply(&rep, &mut out);

        assert_eq!(
            out.axes[0],
            (0.5 - 0.04) / 0.96,
            "deadzone rescales: full-range stays full after edge trim"
        );
        assert!(out.axes[1] < -0.2); // pitch (deadzone edge rescale)
        assert_eq!(out.axes[2], 1.0); // yaw saturates
        assert_eq!(out.axes[3], 0.0); // throttle -1 → 0.0
    }

    #[test]
    fn deadzone_collapses_jitter_and_rescales_edge() {
        let m = ControllerMap::default(); // dz = 0.04
        let mut out = ControlCommand::zeroed(Mode::FullTeleop);
        m.apply(&report([0.02, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]), &mut out);
        assert_eq!(out.axes[0], 0.0, "inside deadzone collapses");

        let mut out2 = ControlCommand::zeroed(Mode::FullTeleop);
        m.apply(&report([1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]), &mut out2);
        assert_eq!(out2.axes[0], 1.0, "full deflection stays full");
    }

    #[test]
    fn invert_flags_negate_channels() {
        let m = ControllerMap {
            invert_pitch: true,
            ..ControllerMap::default()
        };
        let mut out = ControlCommand::zeroed(Mode::FullTeleop);
        m.apply(&report([0.0, 0.5, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]), &mut out);
        assert_eq!(out.axes[1], -(0.5 - 0.04) / 0.96);
    }

    #[test]
    fn throttle_remapped_to_unit_interval() {
        let m = ControllerMap::default();
        let mut out = ControlCommand::zeroed(Mode::FullTeleop);
        m.apply(&report([0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0]), &mut out);
        assert_eq!(out.axes[3], 1.0);
        m.apply(&report([0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]), &mut out);
        assert_eq!(out.axes[3], 0.5);
    }
}
