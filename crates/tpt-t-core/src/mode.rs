//! Operating modes and the legal-transition table (spec §5.4).
//!
//! Modes:
//!
//! * [`Mode::Auto`] — vehicle autonomy in command; operator supervises.
//! * [`Mode::Assist`] — shared control: operator inputs are blended with
//!   autonomy outputs (safety crate applies cubic-spline blending weights).
//! * [`Mode::FullTeleop`] — operator commands drive actuators directly.
//! * [`Mode::EmergencyStop`] — latched hard stop; exit requires explicit
//!   operator acknowledgment into [`Mode::Assist`].
//!
//! The default table forbids direct Auto ↔ FullTeleop jumps: both directions
//! must stage through Assist so the handover can be spline-smoothed instead
//! of snapping. [`StateMachine::force_emergency_stop`](crate::machine::StateMachine::force_emergency_stop)
//! bypasses the table entirely — an E-stop is never refused.

/// Control authority mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Mode {
    /// Vehicle autonomy drives; operator supervises.
    Auto = 0,
    /// Shared control between autonomy and operator.
    Assist = 1,
    /// Operator commands actuate directly.
    FullTeleop = 2,
    /// Latched hard stop; exit requires operator acknowledgment.
    EmergencyStop = 3,
}

impl Mode {
    /// All modes, index-stable with the discriminants.
    pub const ALL: [Mode; 4] = [
        Mode::Auto,
        Mode::Assist,
        Mode::FullTeleop,
        Mode::EmergencyStop,
    ];

    /// Lossless discriminant mapping.
    #[inline]
    pub fn as_u8(self) -> u8 {
        self as u8
    }

    /// Inverse of [`as_u8`](Self::as_u8); `None` for out-of-range values.
    #[inline]
    pub fn from_u8(v: u8) -> Option<Mode> {
        match v {
            0 => Some(Mode::Auto),
            1 => Some(Mode::Assist),
            2 => Some(Mode::FullTeleop),
            3 => Some(Mode::EmergencyStop),
            _ => None,
        }
    }

    /// Human-readable name for logs/HUDs.
    pub fn name(self) -> &'static str {
        match self {
            Mode::Auto => "AUTO",
            Mode::Assist => "ASSIST",
            Mode::FullTeleop => "TELEOP",
            Mode::EmergencyStop => "ESTOP",
        }
    }
}

/// A completed mode change, timestamped (wall-clock nanos).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Transition {
    /// Mode before the change.
    pub from: Mode,
    /// Mode after the change.
    pub to: Mode,
    /// UNIX-epoch nanoseconds of the commit instant.
    pub at_unix_ns: u64,
}

/// Rejected-transition error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModeError {
    /// The table forbids `from → to`.
    Disallowed { from: Mode, to: Mode },
}

impl core::fmt::Display for ModeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ModeError::Disallowed { from, to } => write!(
                f,
                "mode transition {} -> {} is not permitted",
                from.name(),
                to.name()
            ),
        }
    }
}

impl std::error::Error for ModeError {}

/// Dense adjacency matrix of legal transitions.
///
/// Defaults implement the spec's staged-handover policy (see module docs).
#[derive(Debug, Clone)]
pub struct TransitionTable {
    allowed: [[bool; Mode::ALL.len()]; Mode::ALL.len()],
}

impl TransitionTable {
    /// The spec-default policy.
    pub fn spec_default() -> Self {
        let mut t = Self {
            allowed: [[false; 4]; 4],
        };
        // Staged handovers through Assist.
        t.set_allowed(Mode::Auto, Mode::Assist, true);
        t.set_allowed(Mode::Assist, Mode::Auto, true);
        t.set_allowed(Mode::Assist, Mode::FullTeleop, true);
        t.set_allowed(Mode::FullTeleop, Mode::Assist, true);
        // Emergency stop is enterable from anywhere...
        for m in Mode::ALL {
            t.set_allowed(m, Mode::EmergencyStop, true);
        }
        // ...but only exitable into Assist (operator acknowledgment), which
        // then stages onward per normal rules.
        t.set_allowed(Mode::EmergencyStop, Mode::Assist, true);
        t
    }

    /// Reads one edge.
    #[inline]
    pub fn allows(&self, from: Mode, to: Mode) -> bool {
        self.allowed[from.as_u8() as usize][to.as_u8() as usize]
    }

    /// Writes one edge (used by policy tests and future RBAC-driven tables).
    pub fn set_allowed(&mut self, from: Mode, to: Mode, allowed: bool) {
        self.allowed[from.as_u8() as usize][to.as_u8() as usize] = allowed;
    }

    /// Convenience: forbid every edge into `to`.
    pub fn deny_entry(&mut self, to: Mode) {
        for m in Mode::ALL {
            self.set_allowed(m, to, false);
        }
    }
}

impl Default for TransitionTable {
    fn default() -> Self {
        Self::spec_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discriminants_stable() {
        for m in Mode::ALL {
            assert_eq!(Mode::from_u8(m.as_u8()), Some(m));
        }
        assert_eq!(Mode::from_u8(4), None);
    }

    #[test]
    fn default_table_policy() {
        let t = TransitionTable::spec_default();
        assert!(t.allows(Mode::Auto, Mode::Assist));
        assert!(t.allows(Mode::Assist, Mode::FullTeleop));
        assert!(!t.allows(Mode::Auto, Mode::FullTeleop));
        assert!(!t.allows(Mode::FullTeleop, Mode::Auto));
        assert!(t.allows(Mode::FullTeleop, Mode::EmergencyStop));
        assert!(t.allows(Mode::EmergencyStop, Mode::Assist));
        assert!(!t.allows(Mode::EmergencyStop, Mode::FullTeleop));
    }
}
