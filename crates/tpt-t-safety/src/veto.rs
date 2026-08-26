//! Override/veto arbitration for shared human+AI units (spec §5.4 Shared
//! Control).
//!
//! When an AI planning source is present on the vehicle, its safety envelope
//! may **restrict** the human's commands — but per the spec, it may never
//! inject intent of its own. This module encodes exactly that asymmetry:
//!
//! * [`VetoGate::engage`] activates restriction with a translational
//!   magnitude cap.
//! * [`VetoGate::apply`] mutates a command **only downward**: every touched
//!   axis moves strictly toward neutral, never past it, and no axis is ever
//!   pushed away from zero. A disengaged gate is a no-op.
//!
//! State is two atomics — lock-free reads from the RT loop thread.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use tpt_t_core::ser::ControlCommand;

const TRANSLATIONAL: [usize; 4] = [0, 1, 4, 5]; // roll, pitch, lat_x, lat_y

/// Lock-free AI-veto gate.
#[derive(Debug, Default)]
pub struct VetoGate {
    active: AtomicBool,
    /// Translational cap, stored as bit-pattern of f32 (0 = no cap set).
    cap_bits: AtomicU32,
}

// SAFETY: plain atomic flag + boxed-float bits; no cross-field invariants
// require synchronization beyond Relaxed/Acquire-Release used here.
unsafe impl Send for VetoGate {}
unsafe impl Sync for VetoGate {}

impl VetoGate {
    /// Engages the veto with the given normalized translational cap (> 0).
    pub fn engage(&self, max_translational_norm: f32) {
        let cap = max_translational_norm.clamp(0.01, 1.0);
        self.cap_bits.store(cap.to_bits(), Ordering::Release);
        self.active.store(true, Ordering::Release);
    }

    /// Disengages; subsequent [`apply`](Self::apply) calls are no-ops.
    pub fn disengage(&self) {
        self.active.store(false, Ordering::Release);
    }

    /// Whether an AI envelope currently restricts the human.
    #[inline]
    pub fn is_active(&self) -> bool {
        self.active.load(Ordering::Acquire)
    }

    /// Applies the veto to `cmd` in place. Downward-only by construction:
    /// each translational axis is clamped into `[-cap, +cap]`, which can
    /// only shrink magnitude. Returns `true` when anything changed.
    pub fn apply(&self, cmd: &mut ControlCommand) -> bool {
        if !self.is_active() {
            return false;
        }
        let cap = f32::from_bits(self.cap_bits.load(Ordering::Acquire)).max(0.01);
        let mut touched = false;
        for ax in TRANSLATIONAL {
            let before = cmd.axes[ax];
            let after = before.clamp(-cap, cap);
            if after != before {
                cmd.axes[ax] = after;
                touched = true;
            }
        }
        touched
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tpt_t_core::Mode;

    #[test]
    fn disengaged_gate_is_noop() {
        let gate = VetoGate::default();
        let mut cmd = ControlCommand::zeroed(Mode::FullTeleop);
        cmd.axes[4] = 1.0;
        assert!(!gate.apply(&mut cmd));
        assert_eq!(cmd.axes[4], 1.0);
    }

    #[test]
    fn engaged_gate_clamps_but_never_injects() {
        let gate = VetoGate::default();
        gate.engage(0.3);

        let mut cmd = ControlCommand::zeroed(Mode::FullTeleop);
        // Human pushing hard on three axes; yaw (2) and throttle (3) are
        // outside the veto's remit and must stay untouched.
        cmd.axes[0] = -1.0;
        cmd.axes[4] = 1.0;
        cmd.axes[5] = 0.7;
        cmd.axes[2] = 0.9;
        cmd.axes[3] = 0.9;

        assert!(gate.apply(&mut cmd));
        for ax in TRANSLATIONAL {
            assert!(
                cmd.axes[ax].abs() <= 0.3 + 1e-6,
                "ax {ax} = {}",
                cmd.axes[ax]
            );
        }
        assert!((cmd.axes[0] + 0.3).abs() < 1e-6); // direction preserved
        assert_eq!(cmd.axes[2], 0.9, "yaw not vetoed");
        assert_eq!(cmd.axes[3], 0.9, "throttle not vetoed");
        // Nothing grew anywhere.
        assert!(cmd.axes.iter().all(|&a| a.abs() <= 1.0));
    }

    #[test]
    fn smaller_human_input_passes_through_unmodified() {
        let gate = VetoGate::default();
        gate.engage(0.3);
        let mut cmd = ControlCommand::zeroed(Mode::FullTeleop);
        cmd.axes[4] = 0.1;
        assert!(!gate.apply(&mut cmd), "within cap ⇒ untouched");
        assert_eq!(cmd.axes[4], 0.1);
    }
}
