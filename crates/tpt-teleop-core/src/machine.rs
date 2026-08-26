//! The central mode state machine.
//!
//! Mode is stored as an `AtomicU8` so any subsystem can observe it without
//! locks; transitions commit with Release ordering and are recorded into an
//! internal SPSC event ring for observers (HUD, FDR logging) to drain.

use core::sync::atomic::{AtomicU8, Ordering};

use tpt_teleop_ring::SpscRing;

use crate::mode::{Mode, ModeError, Transition, TransitionTable};

/// Default capacity of the transition-event ring.
pub const DEFAULT_EVENT_RING: usize = 64;

/// Lock-free mode state machine.
pub struct StateMachine {
    current: AtomicU8,
    rules: TransitionTable,
    events: SpscRing<Transition>,
}

impl StateMachine {
    /// New machine with the spec-default transition table.
    pub fn new() -> Self {
        Self::with_event_ring(DEFAULT_EVENT_RING)
    }

    /// New machine with a custom event-ring capacity.
    pub fn with_event_ring(event_capacity: usize) -> Self {
        Self {
            current: AtomicU8::new(Mode::Auto.as_u8()),
            rules: TransitionTable::spec_default(),
            events: SpscRing::with_capacity(event_capacity),
        }
    }

    /// Current mode. Unknown discriminants are impossible because only the
    /// transition methods write here; a corrupted value falls back to E-stop.
    #[inline]
    pub fn current(&self) -> Mode {
        match Mode::from_u8(self.current.load(Ordering::Acquire)) {
            Some(m) => m,
            None => Mode::EmergencyStop,
        }
    }

    /// Attempts `current → to` under the transition table.
    /// On success returns the previous mode and records a [`Transition`].
    pub fn try_transition(&self, to: Mode) -> Result<Mode, ModeError> {
        let from = self.current();
        if !self.rules.allows(from, to) {
            return Err(ModeError::Disallowed { from, to });
        }
        self.commit(from, to);
        Ok(from)
    }

    /// Emergency stop: unconditional, bypasses the table. Returns the
    /// previous mode. Callable from any thread, repeatedly (idempotent).
    pub fn force_emergency_stop(&self) -> Mode {
        let from = self.current();
        if from == Mode::EmergencyStop {
            return from;
        }
        self.commit(from, Mode::EmergencyStop);
        from
    }

    /// Drains pending transitions into `out` FIFO; returns count.
    /// Called by HUD/FDR consumers on their own cadence.
    pub fn take_transitions(&self, out: &mut Vec<Transition>) -> usize {
        let mut n = 0;
        while let Some(t) = self.events.pop() {
            out.push(t);
            n += 1;
        }
        n
    }

    fn commit(&self, from: Mode, to: Mode) {
        self.current.store(to.as_u8(), Ordering::Release);
        // Event ring full ⇒ drop the record; authoritative state is the
        // atomic, events are best-effort telemetry.
        let _ = self.events.push(Transition {
            from,
            to,
            at_unix_ns: unix_ns_now(),
        });
    }
}

impl Default for StateMachine {
    fn default() -> Self {
        Self::new()
    }
}

impl core::fmt::Debug for StateMachine {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("StateMachine")
            .field("current", &self.current())
            .finish()
    }
}

fn unix_ns_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn staged_handover_path() {
        let sm = StateMachine::new();
        assert_eq!(sm.current(), Mode::Auto);
        assert_eq!(sm.try_transition(Mode::Assist), Ok(Mode::Auto));
        assert_eq!(sm.try_transition(Mode::FullTeleop), Ok(Mode::Assist));
        assert_eq!(sm.current(), Mode::FullTeleop);

        let mut ev = Vec::new();
        assert_eq!(sm.take_transitions(&mut ev), 2);
        assert_eq!(ev[0].from, Mode::Auto);
        assert_eq!(ev[0].to, Mode::Assist);
        assert_eq!(ev[1].to, Mode::FullTeleop);
        assert!(ev[1].at_unix_ns >= ev[0].at_unix_ns);
    }

    #[test]
    fn direct_teleop_jump_rejected_and_state_unchanged() {
        let sm = StateMachine::new();
        assert_eq!(
            sm.try_transition(Mode::FullTeleop),
            Err(ModeError::Disallowed {
                from: Mode::Auto,
                to: Mode::FullTeleop
            })
        );
        assert_eq!(sm.current(), Mode::Auto);
    }

    #[test]
    fn emergency_stop_is_unconditional_then_latched() {
        let sm = StateMachine::new(); // starts in Auto
        assert_eq!(sm.force_emergency_stop(), Mode::Auto);
        assert_eq!(sm.force_emergency_stop(), Mode::EmergencyStop); // idempotent
        // Latched: everything except Assist ack is refused.
        assert!(sm.try_transition(Mode::FullTeleop).is_err());
        assert!(sm.try_transition(Mode::Auto).is_err());
        assert_eq!(sm.try_transition(Mode::Assist), Ok(Mode::EmergencyStop));
        assert_eq!(sm.current(), Mode::Assist);
    }

    #[test]
    fn concurrent_transition_storm_stays_legal() {
        let sm = Arc::new(StateMachine::with_event_ring(4096));
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));

        // One mutator thread: E-stop is legal from everywhere at all times,
        // so any observed mode pair must be a table-legal edge or an E-stop.
        let mutator = {
            let sm = Arc::clone(&sm);
            let stop = Arc::clone(&stop);
            std::thread::spawn(move || {
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    sm.force_emergency_stop();
                    let _ = sm.try_transition(Mode::Assist);
                    let _ = sm.try_transition(Mode::Auto);
                }
            })
        };

        // Concurrent observers see only valid modes, never torn values.
        let mut observers = Vec::new();
        for _ in 0..3 {
            let sm = Arc::clone(&sm);
            observers.push(std::thread::spawn(move || {
                let deadline = std::time::Instant::now() + std::time::Duration::from_millis(120);
                while std::time::Instant::now() < deadline {
                    let m = sm.current();
                    assert!(Mode::from_u8(m.as_u8()).is_some());
                    std::hint::spin_loop();
                }
            }));
        }
        for o in observers {
            o.join().unwrap();
        }
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        mutator.join().unwrap();

        // Drained events (best-effort telemetry; may have gaps from ring
        // overflow) must each individually be legal edges of the table.
        let rules = TransitionTable::spec_default();
        let mut drained = Vec::new();
        sm.take_transitions(&mut drained);
        assert!(!drained.is_empty());
        for t in &drained {
            assert!(
                rules.allows(t.from, t.to),
                "illegal edge recorded: {} -> {}",
                t.from.name(),
                t.to.name()
            );
        }
    }
}
