//! Cubic/quintic spline smoothing for autonomy-handover transitions (spec
//! §5.4: "mathematically smoothed transitions between Auto/Assist/Teleop").
//!
//! Authority — the mix weight between autonomy baseline and operator input —
//! never snaps between modes. Mode changes start a quintic blend:
//!
//! ```text
//! w(t) = w_from + (w_to − w_from) · s(t/T),   s(x)=6x⁵−15x⁴+10x³
//! ```
//!
//! `s` has zero first **and** second derivatives at both ends, so command
//! acceleration and jerk stay continuous through every handover.

use tpt_t_core::{Mode, ModeError};

/// Quintic smootherstep: C²-continuous at both endpoints.
#[inline]
pub fn smootherstep(x: f64) -> f64 {
    let x = x.clamp(0.0, 1.0);
    x * x * x * (x * (x * 6.0 - 15.0) + 10.0)
}

/// Target authority fraction per operating mode.
pub fn authority_target(mode: Mode) -> f32 {
    match mode {
        Mode::Auto => 0.0,          // autonomy fully in command
        Mode::Assist => 0.5,        // blended shared control
        Mode::FullTeleop => 1.0,    // operator fully in command
        Mode::EmergencyStop => 0.0, // irrelevant: E-stop overrides output
    }
}

/// Animated C² authority blend from one target to another.
#[derive(Debug, Clone)]
pub struct AuthorityBlend {
    from_weight: f32,
    to_weight: f32,
    duration_s: f32,
    elapsed_s: f32,
}

impl AuthorityBlend {
    /// Starts a blend lasting `duration_s`.
    pub fn new(from_weight: f32, to_weight: f32, duration_s: f32) -> Self {
        Self {
            from_weight,
            to_weight,
            duration_s: duration_s.max(1e-3),
            elapsed_s: 0.0,
        }
    }

    /// Advances time by `dt_s`; returns the current authority weight.
    pub fn advance(&mut self, dt_s: f32) -> f32 {
        self.elapsed_s = (self.elapsed_s + dt_s).min(self.duration_s);
        self.weight()
    }

    /// Current weight without advancing.
    #[inline]
    pub fn weight(&self) -> f32 {
        let s = smootherstep(self.elapsed_s as f64 / self.duration_s as f64) as f32;
        self.from_weight + (self.to_weight - self.from_weight) * s
    }

    /// True when the blend reached its target.
    #[inline]
    pub fn done(&self) -> bool {
        self.elapsed_s >= self.duration_s
    }
}

/// Validates that a requested transition is legal per the core state machine
/// before any blending starts (thin wrapper keeping safety-side policy in one
/// place).
pub fn check_transition(
    current: Mode,
    requested: Mode,
    table_allows: impl FnOnce(Mode, Mode) -> bool,
) -> Result<(), ModeError> {
    if table_allows(current, requested) {
        Ok(())
    } else {
        Err(ModeError::Disallowed {
            from: current,
            to: requested,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smootherstep_endpoints_and_monotonicity() {
        assert_eq!(smootherstep(0.0), 0.0);
        assert_eq!(smootherstep(1.0), 1.0);
        let mut prev = -1.0f64;
        for i in 0..=100 {
            let v = smootherstep(i as f64 / 100.0);
            assert!(v >= prev, "must be monotonic at {i}");
            prev = v;
        }
    }

    #[test]
    fn zero_derivatives_at_ends_c2_continuous() {
        // Quintic starts as x³·(10−15x+6x²): near 0 the function value is
        // O(h³) — three orders flatter than cubic smoothstep's O(h²).
        let h = 1e-6;
        let quintic = smootherstep(h);
        let cubic = 3.0 * h * h - 2.0 * h * h * h;
        assert!(
            quintic < cubic / 100.0,
            "quintic {} vs cubic {}",
            quintic,
            cubic
        );
        assert!(quintic < 1e-12);

        // Same flatness at the far end.
        let tail = smootherstep(1.0 - h);
        assert!((1.0 - tail) < 1e-12);
    }

    #[test]
    fn authority_blend_reaches_target_exactly() {
        let mut b = AuthorityBlend::new(0.0, 1.0, 1.0);
        let mut last = b.advance(0.0);
        let mut guard = 0;
        while !b.done() {
            let w = b.advance(0.01); // f32 accumulation-safe: done() caps elapsed
            assert!(w >= last - 1e-6, "authority must not regress");
            last = w;
            guard += 1;
            assert!(guard < 500, "blend never completed");
        }
        assert!((b.weight() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn mode_targets_match_spec_semantics() {
        assert_eq!(authority_target(Mode::Auto), 0.0);
        assert_eq!(authority_target(Mode::Assist), 0.5);
        assert_eq!(authority_target(Mode::FullTeleop), 1.0);
    }

    #[test]
    fn illegal_transition_rejected_by_checker() {
        assert!(
            check_transition(Mode::Auto, Mode::Assist, |f, t| t.as_u8() == f.as_u8() + 1).is_ok()
        );
        assert!(check_transition(Mode::Auto, Mode::FullTeleop, |_, _| false).is_err());
    }
}
