//! AI command sources — first-class producers alongside HID devices
//! (spec §5.1 AI Input Source), always origin-tagged so downstream stages
//! can distinguish machine-authored from human-authored intent.

use tpt_t_core::ser::ControlCommand;

/// Where a command originated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// Human-operated device (HID joystick, VR controller, …).
    Human,
    /// On-unit AI planner / autonomy stack.
    Ai,
}

impl Origin {
    /// Flag bits this origin raises on [`ControlCommand::flags`](tpt_t_core::ser::ControlCommand).
    pub fn flag_bits(self) -> u8 {
        match self {
            // Human is the default (no bits).
            Origin::Human => 0,
            Origin::Ai => tpt_t_core::ser::FLAG_AI_ORIGIN,
        }
    }
}

/// A producer of complete commands (AI planner, autonomy stack, replay…).
///
/// Mirrors [`crate::source::RawInputSource`] but at command granularity:
/// implementations are expected to be polled once per loop tick and must
/// never block or allocate.
pub trait CommandSource: Send {
    /// Produces the next command; `None` when idle this tick.
    fn next_command(&mut self, now_ns: u64) -> Option<ControlCommand>;

    /// Origin tag this source stamps onto every command it emits.
    fn origin(&self) -> Origin;
}

/// Closure-backed AI source: wraps any `(u64) -> ControlCommand` generator.
pub struct AiCommandSource<F>
where
    F: FnMut(u64) -> ControlCommand + Send,
{
    generate: F,
}

impl<F> AiCommandSource<F>
where
    F: FnMut(u64) -> ControlCommand + Send,
{
    /// Wraps a generator closure.
    pub fn new(generate: F) -> Self {
        Self { generate }
    }
}

impl<F> CommandSource for AiCommandSource<F>
where
    F: FnMut(u64) -> ControlCommand + Send,
{
    fn next_command(&mut self, now_ns: u64) -> Option<ControlCommand> {
        let mut cmd = (self.generate)(now_ns);
        cmd.set_ai_origin();
        Some(cmd)
    }

    fn origin(&self) -> Origin {
        Origin::Ai
    }
}
