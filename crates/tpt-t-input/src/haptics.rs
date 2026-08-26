//! Haptic / force-feedback routing (spec §5.1).
//!
//! Safety-relevant events (E-stop latch, geofence warnings) pulse every
//! connected sink through a fan-out router; sinks are the only OS-touching
//! piece and stay swappable (`MockHaptics` for tests, real rumble writers
//! behind the same trait later).

use tpt_t_hal::HalError;

/// Force-feedback command.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum HapticEffect {
    /// Dual-motor rumble: strong + weak motor gains (0..1), duration ms.
    Rumble {
        strong: f32,
        weak: f32,
        duration_ms: u16,
    },
}

impl HapticEffect {
    /// Short strong pulse used for E-stop / hard-breach alerts.
    pub const ALERT: HapticEffect = HapticEffect::Rumble {
        strong: 1.0,
        weak: 0.6,
        duration_ms: 250,
    };
    /// Soft double-tap used for soft-warn proximity hints.
    pub const WARN: HapticEffect = HapticEffect::Rumble {
        strong: 0.25,
        weak: 0.1,
        duration_ms: 80,
    };
}

/// A device that can render haptic effects.
pub trait HapticSink: Send {
    /// Renders one effect; `Err` surfaces transport failures without
    /// blocking the router.
    fn play(&mut self, effect: &HapticEffect) -> Result<(), HalError>;
}

// Blanket impl so `&mut sink` boxes work as trait objects.
impl<T: HapticSink + ?Sized> HapticSink for &mut T {
    fn play(&mut self, effect: &HapticEffect) -> Result<(), HalError> {
        (**self).play(effect)
    }
}

/// Test sink recording everything it was handed (bounded log).
#[derive(Debug, Default)]
pub struct MockHaptics {
    log: Vec<HapticEffect>,
    cap: usize,
}

impl MockHaptics {
    /// Bounded recorder keeping the most recent `cap` effects.
    pub fn new(cap: usize) -> Self {
        Self {
            log: Vec::new(),
            cap: cap.max(1),
        }
    }

    /// Snapshot of recorded effects (oldest first).
    pub fn recorded(&self) -> &[HapticEffect] {
        &self.log
    }
}

impl HapticSink for MockHaptics {
    fn play(&mut self, effect: &HapticEffect) -> Result<(), HalError> {
        if self.log.len() == self.cap {
            self.log.remove(0);
        }
        self.log.push(*effect);
        Ok(())
    }
}

/// Fans safety events out to every registered sink.
#[derive(Default)]
pub struct HapticRouter {
    sinks: Vec<Box<dyn HapticSink>>,
}

impl HapticRouter {
    /// Empty router.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers another sink.
    pub fn add_sink(&mut self, sink: Box<dyn HapticSink>) {
        self.sinks.push(sink);
    }

    /// Broadcasts one effect to all sinks; per-sink failures are ignored by
    /// design (haptics must never stall the loop).
    pub fn broadcast(&mut self, effect: &HapticEffect) {
        for s in &mut self.sinks {
            let _ = s.play(effect);
        }
    }
}
