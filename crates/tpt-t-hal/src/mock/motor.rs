//! Deterministic simulated ESC/motor with first-order dynamics.

use crate::motor::Motor;
use crate::types::{MotorCommand, MotorMode, MotorTelemetry};

/// Rated no-load rotor speed (rad/s) of the simulated powertrain.
pub const MAX_ROTOR_SPEED: f32 = 300.0;
/// First-order time constant of the rotor speed response (s).
pub const TAU_S: f32 = 0.15;
/// Ambient temperature (°C).
pub const AMBIENT_C: f32 = 25.0;

/// Simulated brushless motor channel.
///
/// Dynamics are a deterministic first-order lag toward the commanded
/// setpoint plus a simple thermal model — accurate enough for control-loop
/// tests, cheap enough to run millions of steps. No randomness anywhere.
#[derive(Debug)]
pub struct SimMotor {
    last_cmd: MotorCommand,
    omega: f32, // rad/s
    temp_c: f32,
    volts: f32,
    steps: u64,
}

impl SimMotor {
    /// Fresh motor at rest, ambient temperature, nominal supply.
    pub fn new(volts: f32) -> Self {
        Self {
            last_cmd: MotorCommand::idle(0, 0),
            omega: 0.0,
            temp_c: AMBIENT_C,
            volts,
            steps: 0,
        }
    }

    /// Advances dynamics by `dt_s`. Call once per control tick before
    /// [`read`](Motor::read).
    pub fn tick(&mut self, dt_s: f32) {
        let target = match MotorMode::from_u8(self.last_cmd.mode) {
            Some(MotorMode::Speed) => self.last_cmd.value.clamp(-MAX_ROTOR_SPEED, MAX_ROTOR_SPEED),
            Some(MotorMode::Thrust) => {
                // Thrust ∝ ω², so a commanded thrust *fraction v* settles at
                // ω = √v · ω_max (steady-state thrust == commanded fraction).
                self.last_cmd.value.clamp(0.0, 1.0).sqrt() * MAX_ROTOR_SPEED
            }
            Some(MotorMode::Idle) | None => 0.0,
        };
        // First-order lag: ω += (ω* − ω) · dt/τ  (stable for dt ≤ τ).
        let alpha = (dt_s / TAU_S).min(1.0);
        self.omega += (target - self.omega) * alpha;
        // Thermal: rises with |ω| load, decays to ambient.
        let heat = (self.omega.abs() / MAX_ROTOR_SPEED) * 30.0 - (self.temp_c - AMBIENT_C);
        self.temp_c += heat * alpha * 0.1;
        self.steps += 1;
    }

    /// Instantaneous rotor speed (rad/s).
    #[inline]
    pub fn omega(&self) -> f32 {
        self.omega
    }

    /// Normalized thrust contribution 0..=1 (∝ ω², saturating at rated ω).
    #[inline]
    pub fn normalized_thrust(&self) -> f32 {
        let x = self.omega / MAX_ROTOR_SPEED;
        x * x
    }
}

impl Motor for SimMotor {
    fn apply(&mut self, cmd: &MotorCommand) {
        self.last_cmd = *cmd;
    }

    fn read(&mut self, out: &mut MotorTelemetry) {
        out.seq = self.last_cmd.seq;
        out.timestamp_ns = self.last_cmd.timestamp_ns + self.steps * 5_000_000; // 5 ms ticks
        out.rpm = self.omega;
        out.temp_c = self.temp_c;
        out.volts = self.volts;
        out.errors = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DT: f32 = 0.005; // 200 Hz

    #[test]
    fn first_order_response_is_monotonic_and_bounded() {
        let mut m = SimMotor::new(16.0);
        m.apply(&MotorCommand {
            seq: 1,
            timestamp_ns: 0,
            mode: MotorMode::Thrust.as_u8(),
            _reserved: [0; 3],
            value: 1.0,
        });

        let mut prev = m.omega();
        let mut rising = true;
        for _ in 0..200 {
            m.tick(DT);
            let w = m.omega();
            assert!((0.0..=MAX_ROTOR_SPEED).contains(&w), "ω out of range: {w}");
            if w < prev - 1e-6 {
                rising = false;
            }
            prev = w;
        }
        assert!(rising, "speed must be monotonic toward target");
        assert!(prev > MAX_ROTOR_SPEED * 0.95, "should reach ~rated speed");
    }

    #[test]
    fn idle_command_decays_to_rest() {
        let mut m = SimMotor::new(16.0);
        m.apply(&MotorCommand {
            seq: 1,
            timestamp_ns: 0,
            mode: MotorMode::Thrust.as_u8(),
            _reserved: [0; 3],
            value: 1.0,
        });
        for _ in 0..400 {
            m.tick(DT);
        }
        m.apply(&MotorCommand::idle(2, 0));
        for _ in 0..600 {
            m.tick(DT);
        }
        assert!(m.omega() < 1.0, "idle must decay, got {}", m.omega());
    }

    #[test]
    fn read_reports_state_without_advancing_time() {
        let mut m = SimMotor::new(14.8);
        m.apply(&MotorCommand {
            seq: 7,
            timestamp_ns: 100,
            mode: MotorMode::Speed.as_u8(),
            _reserved: [0; 3],
            value: 150.0,
        });
        m.tick(DT);
        let mut t = MotorTelemetry::default();
        m.read(&mut t);
        let snap = t.rpm;
        m.read(&mut t);
        assert_eq!(t.rpm, snap, "read must not advance dynamics");
        assert_eq!(t.seq, 7);
        assert_eq!(t.errors, 0);
    }

    #[test]
    fn thrust_normalization_matches_speed_squared() {
        let mut m = SimMotor::new(16.0);
        m.apply(&MotorCommand {
            seq: 1,
            timestamp_ns: 0,
            mode: MotorMode::Thrust.as_u8(),
            _reserved: [0; 3],
            value: 0.5,
        });
        for _ in 0..400 {
            m.tick(DT);
        }
        let expected = 0.5f32; // steady-state thrust == commanded fraction
        assert!((m.normalized_thrust() - expected).abs() < 0.02);
    }
}
