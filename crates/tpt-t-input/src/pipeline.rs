//! Input ingest pipeline: source → zero-copy map → ring → (safety loop).

use std::sync::Arc;

use tpt_t_core::ser::ControlCommand;
use tpt_t_ring::SpscRing;

use crate::ai_source::{CommandSource, Origin};
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
        cmd.clear_ai_origin();
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

/// Stage for non-human producers (AI planner / autonomy stack). Commands
/// arrive already tagged with their origin and flow into the same ring the
/// safety loop drains — identical plumbing, per spec §5.1 AI Input Source.
pub struct CommandStage<S: CommandSource> {
    source: S,
    ring: Arc<SpscRing<ControlCommand>>,
}

impl<S: CommandSource> CommandStage<S> {
    /// Wires the source into `ring`.
    pub fn new(source: S, ring: Arc<SpscRing<ControlCommand>>) -> Self {
        Self { source, ring }
    }

    /// Polls the source; on a fresh command, stamps the tick timestamp and
    /// queues it. Returns the command when one was produced.
    pub fn tick(&mut self, now_ns: u64) -> Option<ControlCommand> {
        let mut cmd = self.source.next_command(now_ns)?;
        cmd.timestamp_ns = now_ns;
        self.ring.push(cmd).ok()?;
        Some(cmd)
    }

    /// Origin tag of the wrapped source.
    pub fn origin(&self) -> Origin {
        self.source.origin()
    }
}
