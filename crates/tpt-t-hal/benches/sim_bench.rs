//! Physics-simulator step throughput/latency benchmark for the HAL
//! (Phase 14, item 5). Measures one fixed-step `QuadDrone` integration through
//! the mock CAN actuator channel — the Phase 4 simulator's per-tick cost, which
//! underpins every end-to-end integration test.

use std::hint::black_box;
use std::time::Instant;

use tpt_t_core::ser::{GpsSample, ImuSample};
use tpt_t_hal::mock::CanEndpoint;
use tpt_t_hal::sim::{DT_S, QuadDrone, World};
use tpt_t_hal::{CanBus, CanFrame, Pose6D, can_pair, ids};

fn motor_frame(thrust: f32) -> CanFrame {
    let raw = (thrust.clamp(0.0, 1.0) * u16::MAX as f32) as u16;
    let be = raw.to_be_bytes();
    let mut payload = [0u8; 8];
    for pair in payload.chunks_mut(2) {
        pair[0] = be[0];
        pair[1] = be[1];
    }
    CanFrame::new(ids::MOTOR_CMD, &payload)
}

struct Sim {
    world: World,
    drone: QuadDrone,
    can_op: CanEndpoint,
    can_veh: CanEndpoint,
    imu: ImuSample,
    gps: GpsSample,
    pose: Pose6D,
}

impl Sim {
    fn new() -> Self {
        let mut world = World::new([0.0, 0.0, -9.81]);
        let drone = QuadDrone::spawn(&mut world);
        let (can_op, can_veh) = can_pair(16);
        Self {
            world,
            drone,
            can_op,
            can_veh,
            imu: ImuSample::zeroed(0, 0),
            gps: GpsSample::zeroed(0, 0),
            pose: Pose6D::default(),
        }
    }

    fn step(&mut self, thrust: f32) {
        let _ = self.can_op.send(&motor_frame(thrust));
        loop {
            let mut f = CanFrame::new(0, &[]);
            if !self.can_veh.recv(&mut f) {
                break;
            }
            self.drone.handle_can(&f);
        }
        self.drone.apply_actuation(&mut self.world, DT_S);
        self.world.step(DT_S);
        self.drone.post_step(
            &self.world,
            DT_S,
            &mut self.imu,
            &mut self.gps,
            &mut self.pose,
        );
    }
}

fn pct(s: &[u64], p: f64) -> u64 {
    let i = ((s.len() - 1) as f64 * p).round() as usize;
    s[i]
}

fn main() {
    const ITER: u64 = 100_000;
    let mut sim = Sim::new();

    let mut lat = Vec::with_capacity(ITER as usize);
    let t0 = Instant::now();
    for _ in 0..ITER {
        let start = Instant::now();
        sim.step(black_box(0.5));
        lat.push(start.elapsed().as_nanos() as u64);
    }
    let el = t0.elapsed();
    lat.sort_unstable();

    let tp = ITER as f64 / el.as_secs_f64();
    println!(
        "hal sim step: n={} p50={}ns p99={}ns p99.9={}ns max={}ns throughput={:.0} steps/s",
        ITER,
        pct(&lat, 0.50),
        pct(&lat, 0.99),
        pct(&lat, 0.999),
        lat[lat.len() - 1],
        tp
    );
}
