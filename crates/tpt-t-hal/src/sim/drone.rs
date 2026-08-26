//! Quadrotor fixture: mock ESCs + virtual CAN + sensors + physics core.

use tpt_t_core::ser::{GpsSample, ImuSample};

use crate::can::ids;
use crate::mock::motor::SimMotor;
use crate::motor::Motor;
use crate::sim::world::{BodyId, World};
use crate::types::{CanFrame, MotorCommand, MotorMode, Pose6D};

/// Number of rotors.
pub const N_MOTORS: usize = 4;
/// Rotor distance from center of mass (m).
pub const ARM_M: f64 = 0.22;
/// Airframe mass (kg).
pub const MASS_KG: f64 = 1.1;
/// Maximum thrust per fully-saturated rotor (N). Four rotors at ~55 % hover.
pub const MAX_ROTOR_THRUST_N: f64 = 5.0;
/// Reaction-torque coefficient mapping rotor speed asymmetry to yaw torque.
const YAW_COEF: f64 = 0.0009;

/// Rotor layout `[FL, FR, RL, RR]`, spin signs `(+, −, −, +)` as `[x, y]` m.
pub const ROTOR_POS: [[f64; 2]; N_MOTORS] = [
    [-ARM_M, ARM_M],
    [ARM_M, ARM_M],
    [-ARM_M, -ARM_M],
    [ARM_M, -ARM_M],
];

/// A simulated quadrotor attached to a [`World`] body. Fixture contract:
/// [`handle_can`](Self::handle_can) frames from the virtual bus,
/// [`apply_actuation`](Self::apply_actuation) before `World::step`, then
/// [`post_step`](Self::post_step) to refresh sensor records.
pub struct QuadDrone {
    body: BodyId,
    motors: [SimMotor; N_MOTORS],
    prev_vel: [f64; 3],
    cmd_seq: u64,
}

impl QuadDrone {
    /// Spawns the vehicle just above the ground plane.
    pub fn spawn(world: &mut World) -> Self {
        let half = [0.12, 0.12, 0.045];
        let body = world.add_box(MASS_KG, half, [0.0, 0.0, half[2] + 0.02], 0.0);
        Self {
            body,
            motors: std::array::from_fn(|_| SimMotor::new(16.0)),
            prev_vel: [0.0; 3],
            cmd_seq: 1,
        }
    }

    /// Parses a motor-command CAN frame (`ids::MOTOR_CMD`, 4 × u16 BE
    /// normalized thrust). Returns `true` when consumed.
    pub fn handle_can(&mut self, frame: &CanFrame) -> bool {
        if frame.id != ids::MOTOR_CMD || frame.len < 8 {
            return false;
        }
        for i in 0..N_MOTORS {
            let raw = u16::from_be_bytes([frame.data[i * 2], frame.data[i * 2 + 1]]);
            self.motors[i].apply(&MotorCommand {
                seq: self.cmd_seq,
                timestamp_ns: self.cmd_seq.wrapping_mul(5_000_000),
                mode: MotorMode::Thrust.as_u8(),
                _reserved: [0; 3],
                value: raw as f32 / u16::MAX as f32,
            });
        }
        self.cmd_seq += 1;
        true
    }

    /// Per-motor normalized thrust snapshot (post-dynamics).
    pub fn thrusts(&self) -> [f32; N_MOTORS] {
        std::array::from_fn(|i| self.motors[i].normalized_thrust())
    }

    /// Integrates ESC dynamics and accumulates forces/torques into `world`.
    /// Call once per control tick **before** `world.step(dt)`.
    pub fn apply_actuation(&mut self, world: &mut World, dt_s: f64) {
        for m in &mut self.motors {
            m.tick(dt_s as f32);
        }

        // Body-frame resultant from the rotor layout.
        let mut fz = 0.0f64;
        let (mut tau_x, mut tau_y) = (0.0f64, 0.0f64);
        for (pos, motor) in ROTOR_POS.iter().zip(&self.motors) {
            let fi = motor.normalized_thrust() as f64 * MAX_ROTOR_THRUST_N;
            fz += fi;
            tau_x += pos[1] * fi; // roll arm
            tau_y += -pos[0] * fi; // pitch arm
        }
        let sum = |idx: &[usize]| {
            idx.iter()
                .map(|&i| self.motors[i].omega() as f64)
                .sum::<f64>()
        };
        let tau_z = YAW_COEF * (sum(&[0, 3]) - sum(&[1, 2]));

        // Thrust acts along body +Z; rotate to world frame.
        let up_world = world.rotate_to_world(self.body, &[0.0, 0.0, fz]);
        world.add_force(self.body, up_world);
        world.add_torque(self.body, [tau_x, tau_y, tau_z]);
    }

    /// Refreshes sensor records after `world.step` (`dt` must match).
    pub fn post_step(
        &mut self,
        world: &World,
        dt: f64,
        imu: &mut ImuSample,
        gps: &mut GpsSample,
        pose: &mut Pose6D,
    ) {
        let s = world.get(self.body);
        let acc = [
            (s.vel[0] - self.prev_vel[0]) / dt,
            (s.vel[1] - self.prev_vel[1]) / dt,
            (s.vel[2] - self.prev_vel[2]) / dt,
        ];
        self.prev_vel = s.vel;

        // IMU measures proper acceleration (a − g) in the body frame.
        let spec_world = [acc[0], acc[1], acc[2] + 9.81];
        let spec_body = world.rotate_to_body(self.body, &spec_world);
        imu.seq += 1;
        imu.gyro_rps = [s.omega[0] as f32, s.omega[1] as f32, s.omega[2] as f32];
        imu.accel_g = [
            (spec_body[0] / 9.81) as f32,
            (spec_body[1] / 9.81) as f32,
            (spec_body[2] / 9.81) as f32,
        ];

        // GPS: flat-earth approximation around a fixed datum.
        const LAT0: f64 = 47.6062;
        const LON0: f64 = -122.3321;
        gps.seq += 1;
        gps.lat_deg = LAT0 + s.pos[0] / 111_320.0;
        gps.lon_deg = LON0 + s.pos[1] / (111_320.0 * LAT0.to_radians().cos());
        gps.alt_m = s.pos[2];
        let ground_speed = s.vel[0].hypot(s.vel[1]);
        gps.speed_mps = ground_speed as f32;
        if ground_speed > 0.1 {
            gps.course_deg = (s.vel[1].atan2(s.vel[0])).to_degrees().rem_euclid(360.0) as f32;
        }
        gps.sats = 14;
        gps.fix_ok = 1;

        let (yaw, pitch, roll) = world.euler(self.body);
        *pose = Pose6D {
            x: s.pos[0],
            y: s.pos[1],
            z: s.pos[2],
            yaw: yaw as f32,
            pitch: pitch as f32,
            roll: roll as f32,
            _reserved: 0,
        };
    }
}
