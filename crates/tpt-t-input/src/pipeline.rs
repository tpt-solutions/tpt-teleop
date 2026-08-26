//! Input ingest pipeline: source → zero-copy map → ring → (safety loop).

use std::sync::Arc;

use tpt_t_core::ser::ControlCommand;
use tpt_t_ring::SpscRing;

use crate::haptics::{HapticEffect, HapticRouter};
use crate::map::ControllerMap;
use crate::report::ControllerReport;
use crate::source::RawInputSource;

/// One wired input channel: a raw device source mapped onto a command ring.
///
/// `tick` is called from the input role thread each loop period; it polls
/// the device once and, on a fresh report, maps it into a stack-allocated
/// [`ControlCommand`] and pushes it into the output ring for the safety
/// loop. Haptic alerts ride along: safety flag bits on the *previous*
/// command trigger feedback on the next tick.
pub struct InputStage<S: RawInputSource> {
    source: S,
    map: ControllerMap,
    ring: Arc<SpscRing<ControlCommand>>,
    pub haptics: HapticRouter,
    scratch_report: ControllerReport,
}

impl<S: RawInputSource> InputStage<S> {
    /// Wires `source` through `map` into `ring`.
    pub fn new(source: S, map: ControllerMap, ring: Arc<SpscRing<ControlCommand>>) -> Self {
        Self {
            source,
            map,
            ring,
            haptics: HapticRouter::new(),
            scratch_report: ControllerReport::default(),
        }
    }

    /// Polls and, on fresh input, produces + queues one command.
    /// Returns the command when one was produced this tick.
    pub fn tick(&mut self, now_ns: u64) -> Option<ControlCommand> {
        if !self.source.poll(&mut self.scratch_report) {
            return None;
        }
        let mut cmd = ControlCommand::zeroed(tpt_t_core::Mode::FullTeleop);
        cmd.timestamp_ns = now_ns;
        self.map.apply(&self.scratch_report, &mut cmd);

        // Haptics: alert pattern when safety flags request feedback.
        if cmd.flags & 0b0000_0010 != 0 {
            self.haptics.broadcast(&HapticEffect::WARN);
        }

        self.ring.push(cmd).ok()?;
        Some(cmd)
    }

    /// Immutable view of the mapper.
    pub fn mapper(&self) -> &ControllerMap {
        &self.map
    }
}
