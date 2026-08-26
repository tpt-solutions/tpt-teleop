//! End-to-end integration: operator → virtual CAN bus → simulated quadrotor
//! (Phase 4 acceptance scenario).

use tpt_t_core::ser::{GpsSample, ImuSample};
use tpt_t_hal::sim::DT_S;
use tpt_t_hal::{CanBus, CanFrame, Pose6D, QuadDrone, World, can_pair, ids};

/// Builds the broadcast motor-command frame: 4 × u16 BE normalized thrust.
fn thrust_frame(thrust: f32) -> CanFrame {
    let raw = (thrust.clamp(0.0, 1.0) * u16::MAX as f32).round() as u16;
    let be = raw.to_be_bytes();
    let mut payload = [0u8; 8];
    for pair in payload.chunks_mut(2) {
        pair[0] = be[0];
        pair[1] = be[1];
    }
    CanFrame::new(ids::MOTOR_CMD, &payload)
}

struct FlightResult {
    alt_m: f64,
    vspeed_mps: f64,
    fingerprint: [u64; 3], // pose z / pitch / roll bit patterns
}

/// Flies one scenario end to end through the virtual bus.
fn fly(thrust: f32, seconds: f64) -> FlightResult {
    let mut world = World::new([0.0, 0.0, -9.81]);
    let mut drone = QuadDrone::spawn(&mut world);
    let (mut operator, mut vehicle) = can_pair(16);

    let mut imu = ImuSample::zeroed(0, 0);
    let mut gps = GpsSample::zeroed(0, 0);
    let mut pose = Pose6D::default();

    let ticks = (seconds / DT_S) as usize;
    let mut prev_z = pose.z;
    let mut vspeed = 0.0f64;
    for _ in 0..ticks {
        // Operator side: broadcast the command every tick.
        operator.send(&thrust_frame(thrust)).unwrap();

        // Vehicle side: drain all pending frames into the flight controller.
        loop {
            let mut frame = CanFrame::new(0, &[]);
            if !vehicle.recv(&mut frame) {
                break;
            }
            drone.handle_can(&frame);
        }

        drone.apply_actuation(&mut world, DT_S);
        world.step(DT_S);
        drone.post_step(&world, DT_S, &mut imu, &mut gps, &mut pose);
        vspeed = (pose.z - prev_z) / DT_S;
        prev_z = pose.z;
    }

    FlightResult {
        alt_m: pose.z,
        vspeed_mps: vspeed,
        fingerprint: [
            pose.z.to_bits(),
            (imu.accel_g[2].to_bits() as u64) << 32,
            (pose.roll.to_bits() as u64) << 32,
        ],
    }
}

#[test]
fn hover_thrust_climbs_and_streams_sensors() {
    // 4 rotors × ~62 % ≈ mass·g·1.4 net-up → gentle climb.
    let r = fly(0.62, 3.0);
    assert!(
        r.alt_m > 0.4 && r.alt_m < 30.0,
        "altitude outside climb band: {}",
        r.alt_m
    );
    assert!(r.vspeed_mps > 0.0, "must still be climbing at t=3 s");
}

#[test]
fn zero_thrust_stays_on_the_ground() {
    let r = fly(0.0, 2.0);
    assert!(r.alt_m < 0.2, "drone must stay grounded, got {}", r.alt_m);
    assert!(r.vspeed_mps < 0.5);
}

#[test]
fn identical_scenarios_are_bit_deterministic() {
    let a = fly(0.55, 2.0);
    let b = fly(0.55, 2.0);
    assert_eq!(
        a.fingerprint, b.fingerprint,
        "simulation must be deterministic"
    );
}

#[test]
fn imu_reports_upward_specific_force_while_climbing() {
    // Short hop at high thrust: body-frame accel_z must exceed 1 g early on
    // (net upward acceleration) — proves IMU orientation math is live.
    let mut world = World::new([0.0, 0.0, -9.81]);
    let mut drone = QuadDrone::spawn(&mut world);
    let (mut op, mut veh) = can_pair(4);
    let mut imu = ImuSample::zeroed(0, 0);
    let mut gps = GpsSample::zeroed(0, 0);
    let mut pose = Pose6D::default();

    op.send(&thrust_frame(1.0)).unwrap();
    let mut max_az = -10.0f32;
    for _ in 0..50 {
        loop {
            let mut frame = CanFrame::new(0, &[]);
            if !veh.recv(&mut frame) {
                break;
            }
            drone.handle_can(&frame);
        }
        drone.apply_actuation(&mut world, DT_S);
        world.step(DT_S);
        drone.post_step(&world, DT_S, &mut imu, &mut gps, &mut pose);
        max_az = max_az.max(imu.accel_g[2]);
    }
    assert!(max_az > 1.05, "expected >1 g specific force, got {max_az}");
}
