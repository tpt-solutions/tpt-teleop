//! Latency compensation for stale operator commands.
//!
//! Commands carry capture timestamps; when they arrive at the safety loop,
//! the delay already spent in transit means the vehicle kept executing the
//! previous command longer than the operator assumed. Full state prediction
//! belongs to the estimator; what the loop can do cheaply and deterministically
//! is (a) track the delay EWMA and (b) attenuate aggressive translational
//! commands proportionally to observed latency, so stale inputs land softer.

use tpt_t_core::ser::ControlCommand;

const ROLL: usize = 0;
const PITCH: usize = 1;
const LAT_X: usize = 4;
const LAT_Y: usize = 5;

/// EWMA-based delay tracker with attenuation policy.
#[derive(Debug, Clone)]
pub struct LatencyCompensator {
    ewma_ns: f64,
    alpha: f64,
    max_ns: f64,
    samples: u64,
}

impl LatencyCompensator {
    /// New tracker; `alpha` is the EWMA factor (0..1, higher = snappier),
    /// `max_ns` the delay beyond which attenuation bottoms out.
    pub fn new(alpha: f64, max_ns: u64) -> Self {
        Self {
            ewma_ns: 0.0,
            alpha: alpha.clamp(0.01, 1.0),
            max_ns: max_ns.max(1) as f64,
            samples: 0,
        }
    }

    /// Records one observation (`now − stamp`) clamped to ≥ 0 and returns the
    /// raw delay in nanoseconds.
    pub fn observe(&mut self, now_ns: u64, stamp_ns: u64) -> u64 {
        let delay = now_ns.saturating_sub(stamp_ns);
        let d = delay as f64;
        self.ewma_ns = if self.samples == 0 {
            d
        } else {
            self.ewma_ns * (1.0 - self.alpha) + d * self.alpha
        };
        self.samples += 1;
        delay
    }

    /// Confidence multiplier ∈ [floor, 1] applied to translational axes.
    /// 1.0 while latency is negligible; decays toward a 0.2 floor as the
    /// EWMA approaches `max_ns`.
    pub fn attenuation(&self) -> f32 {
        if self.ewma_ns <= 0.0 {
            return 1.0;
        }
        let conf = 1.0 - (self.ewma_ns / self.max_ns).min(1.0) * 0.8;
        conf as f32
    }

    /// Applies [`attenuation`](Self::attenuation) to the translational axes
    /// of `cmd` in place (attitude/yaw untouched — those are rate commands
    /// where staleness is less dangerous).
    pub fn compensate(&self, cmd: &mut ControlCommand) {
        let k = self.attenuation();
        if k < 1.0 {
            for ax in [ROLL, PITCH, LAT_X, LAT_Y] {
                cmd.axes[ax] *= k;
            }
        }
    }

    /// Current delay estimate (ns).
    #[inline]
    pub fn ewma_ns(&self) -> f64 {
        self.ewma_ns
    }

    /// Observations seen so far.
    #[inline]
    pub fn samples(&self) -> u64 {
        self.samples
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tpt_t_core::Mode;

    #[test]
    fn ewma_converges_toward_recent_delay() {
        let mut lc = LatencyCompensator::new(0.2, 100_000_000);
        for _ in 0..50 {
            lc.observe(1_000_000, 900_000); // steady 100 µs link
        }
        assert!((lc.ewma_ns() - 100_000.0).abs() < 5_000.0);
        // Sudden jump dominates quickly with α=0.2.
        lc.observe(3_000_000, 1_000_000); // 2 ms spike
        assert!(lc.ewma_ns() > 200_000.0);
    }

    #[test]
    fn attenuation_floors_at_twenty_percent() {
        let mut lc = LatencyCompensator::new(0.5, 50_000_000);
        assert_eq!(lc.attenuation(), 1.0);
        for _ in 0..20 {
            lc.observe(60_000_000, 0); // 60 ms ≫ max
        }
        let k = lc.attenuation();
        assert!(k > 0.19 && k < 0.21, "floor ≈0.2, got {k}");
    }

    #[test]
    fn compensate_scales_translational_axes_only() {
        let mut lc = LatencyCompensator::new(0.5, 50_000_000);
        for _ in 0..20 {
            lc.observe(60_000_000, 0);
        }
        let mut cmd = ControlCommand::zeroed(Mode::FullTeleop);
        cmd.axes[0] = 1.0;
        cmd.axes[2] = 1.0; // yaw untouched
        cmd.axes[4] = -0.8;
        lc.compensate(&mut cmd);
        assert!(cmd.axes[0] < 0.99);
        assert_eq!(cmd.axes[2], 1.0);
        assert!(cmd.axes[4] > -0.79 && cmd.axes[4] < -0.15);
    }
}
