//! Phase 6 acceptance: evdev wire-format parsing, co-pilot arbitration,
//! haptics fan-out, and the full chain
//! HID report → zero-copy map → ring → safety loop → simulated quadrotor.

use std::sync::Arc;

use tpt_t_core::ser::{ControlCommand, GpsSample, ImuSample};
use tpt_t_core::{Mode, StateMachine};
use tpt_t_hal::sim::DT_S;
use tpt_t_hal::{CanBus, CanFrame, Pose6D, QuadDrone, World, can_pair, ids};
use tpt_t_input::evdev_parse::{
    ABS_X, EV_ABS, EV_KEY, EV_SYN, EVENT_SIZE, EvdevAccumulator, EvdevEvent,
};
use tpt_t_input::{
    CoPilotHub, ControllerMap, ControllerReport, HapticEffect, HapticRouter, InputStage,
    RawInputSource,
};
use tpt_t_ring::SpscRing;
use tpt_t_safety::{GeoFence, SafetyConfig, SafetyLoop, axis};

// --- evdev wire-format parser ------------------------------------------------

fn ev(sec: i64, usec: i64, kind: u16, code: u16, value: i32) -> [u8; EVENT_SIZE] {
    let mut b = [0u8; EVENT_SIZE];
    b[0..8].copy_from_slice(&sec.to_le_bytes());
    b[8..16].copy_from_slice(&(usec as u64).to_le_bytes());
    b[16..18].copy_from_slice(&kind.to_le_bytes());
    b[18..20].copy_from_slice(&code.to_le_bytes());
    b[20..24].copy_from_slice(&value.to_le_bytes());
    b
}

#[test]
fn evdev_records_decode_and_normalize() {
    let mut acc = EvdevAccumulator::default();
    // Stick full-right on a ±1023 axis.
    acc.push(&EvdevEvent {
        tv_sec: 5,
        tv_usec: 1,
        kind: EV_ABS,
        code: ABS_X,
        value: 1023,
    });
    // Button press (code 304 -> bit 304 % 32 = 16).
    acc.push(&EvdevEvent {
        tv_sec: 5,
        tv_usec: 2,
        kind: EV_KEY,
        code: 304,
        value: 1,
    });
    // SYN frame terminator.
    acc.push(&EvdevEvent {
        tv_sec: 5,
        tv_usec: 3,
        kind: EV_SYN,
        code: 0,
        value: 0,
    });

    assert_eq!(acc.axes[0], 1.0);
    assert_eq!(acc.buttons & (1u32 << 16), 1u32 << 16);
    assert_eq!(acc.last_time.0, 5);

    // Byte-level round trip through the decoder.
    let rec = ev(9, 123_456, EV_ABS, ABS_X, -1023);
    let decoded = tpt_t_input::evdev_parse::decode_event(&rec, 0).unwrap();
    assert_eq!((decoded.tv_sec, decoded.value), (9, -1023));
    assert!(tpt_t_input::evdev_parse::decode_event(&rec[..10], 0).is_none());

    let mut out = ControllerReport::default();
    acc.snapshot(&mut out);
    assert_eq!(out.buttons & (1u32 << 16), 1u32 << 16);
}

#[test]
fn evdev_chunk_of_many_records_parses_in_order() {
    let mut bytes = Vec::new();
    for i in 0..10i64 {
        bytes.extend_from_slice(&ev(i, i * 100, EV_ABS, ABS_X, 0));
        bytes.extend_from_slice(&ev(i, i * 100 + 1, EV_SYN, 0, 0));
    }
    let mut acc = EvdevAccumulator::default();
    let mut off = 0;
    while off + EVENT_SIZE <= bytes.len() {
        if let Some(evd) = tpt_t_input::evdev_parse::decode_event(&bytes, off) {
            acc.push(&evd);
        }
        off += EVENT_SIZE;
    }
    // Centered stick on ±1023 calib ⇒ ~0.
    assert!(acc.axes[0].abs() < 0.01);
    assert_eq!(acc.last_time.0, 9);
}

// --- co-pilot arbitration ------------------------------------------------------

#[test]
fn copilot_primary_wins_then_co_pilot_takes_over_after_expiry() {
    let hub = CoPilotHub::new(100); // 100 ns heartbeat expiry
    hub.heartbeat(0, 10);
    assert_eq!(hub.effective_operator(50), Some(0));
    assert_eq!(hub.authority_for(0, 50), 1.0);
    assert_eq!(hub.authority_for(1, 50), 0.0);

    // Co-pilot starts heartbeating; chief still fresher ⇒ chief keeps control.
    hub.heartbeat(1, 90);
    assert_eq!(hub.effective_operator(95), Some(0));

    // Chief goes silent past the timeout → co-pilot inherits.
    hub.heartbeat(1, 150);
    assert_eq!(hub.effective_operator(200), Some(1));

    // Explicit release beats any heartbeat freshness.
    hub.heartbeat(0, 300);
    hub.release(0);
    assert_ne!(hub.effective_operator(310), Some(0));
}

#[test]
fn no_live_operator_yields_none() {
    let hub = CoPilotHub::new(50);
    hub.heartbeat(0, 0);
    assert_eq!(hub.effective_operator(10_000), None);
}

// --- haptics -----------------------------------------------------------------

struct Counting(std::sync::Arc<std::sync::atomic::AtomicU32>);
impl tpt_t_input::HapticSink for Counting {
    fn play(&mut self, _e: &HapticEffect) -> Result<(), tpt_t_hal::HalError> {
        self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    }
}

#[test]
fn haptic_router_fans_out_to_every_sink() {
    use std::sync::atomic::Ordering;
    let mut router = HapticRouter::new();
    let c1 = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let c2 = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));

    router.add_sink(Box::new(Counting(std::sync::Arc::clone(&c1))));
    router.add_sink(Box::new(Counting(std::sync::Arc::clone(&c2))));

    router.broadcast(&HapticEffect::ALERT);
    router.broadcast(&HapticEffect::WARN);

    assert_eq!(c1.load(Ordering::Relaxed), 2);
    assert_eq!(c2.load(Ordering::Relaxed), 2);
}

// --- AI input source (spec §5.1) ------------------------------------------------

#[test]
fn ai_source_commands_are_origin_tagged_and_flow_through_pipeline() {
    use tpt_t_input::{AiCommandSource, CommandSource, CommandStage};

    // An "AI planner" generating a gentle climb ramp.
    let src = AiCommandSource::new(|now_ns: u64| {
        let mut c = ControlCommand::zeroed(Mode::FullTeleop);
        c.seq = now_ns / 5_000_000;
        c.timestamp_ns = now_ns;
        c.axes[3] = 0.60; // mild climb
        c
    });
    assert_eq!(src.origin(), tpt_t_input::Origin::Ai);

    let ring = Arc::new(SpscRing::<ControlCommand>::with_capacity(16));
    let mut stage = CommandStage::new(src, Arc::clone(&ring));

    let cmd = stage.tick(10_000_000).expect("AI produced a command");
    assert!(cmd.is_ai_origin(), "AI commands must carry the origin flag");
    assert!((cmd.axes[3] - 0.60).abs() < 1e-6);

    // The same command is what downstream pops from the ring.
    let out = ring.pop().unwrap();
    assert!(out.is_ai_origin());
}

#[test]
fn human_stage_commands_are_not_tagged_ai() {
    let reports = (0..8usize)
        .map(|i| ControllerReport {
            seq: i as u32,
            buttons: 0,
            axes: [0.1, 0.0, 0.0, 0.55, 0.0, 0.0, 0.0, 0.0],
            timestamp_ns: i as u64 * 5_000_000,
        })
        .collect();

    let ring = Arc::new(SpscRing::<ControlCommand>::with_capacity(16));
    let source = ScriptedSource::new(reports);
    let mut stage = InputStage::new(source, ControllerMap::default(), Arc::clone(&ring));

    let now = 0u64;
    let _ = stage.tick(now * 5_000_000 + 1);
    let out = ring.pop().unwrap();
    assert!(!out.is_ai_origin(), "human commands must not be AI-tagged");
}

struct ScriptedSource {
    reports: Vec<ControllerReport>,
    idx: usize,
}

impl ScriptedSource {
    fn new(reports: Vec<ControllerReport>) -> Self {
        Self { reports, idx: 0 }
    }
}

impl RawInputSource for ScriptedSource {
    fn poll(&mut self, out: &mut ControllerReport) -> bool {
        if self.idx >= self.reports.len() {
            return false;
        }
        *out = self.reports[self.idx];
        self.idx += 1;
        true
    }

    fn info(&self) -> &tpt_t_input::DeviceInfo {
        unreachable!("fixture carries no identity");
    }

    fn reopen(&mut self) -> Result<(), tpt_t_input::InputError> {
        self.idx = 0;
        Ok(())
    }
}

fn build_motor_frame(thrust: f32) -> CanFrame {
    let raw = (thrust.clamp(0.0, 1.0) * u16::MAX as f32) as u16;
    let be = raw.to_be_bytes();
    let mut payload = [0u8; 8];
    for pair in payload.chunks_mut(2) {
        pair[0] = be[0];
        pair[1] = be[1];
    }
    CanFrame::new(ids::MOTOR_CMD, &payload)
}

#[test]
fn hid_through_safety_loop_flies_sim_inside_envelope() {
    let mut world = World::new([0.0, 0.0, -9.81]);
    let mut drone = QuadDrone::spawn(&mut world);
    let (mut can_op, mut can_veh) = can_pair(16);

    // Hostile operator: saturated tilt + lateral every single tick.
    let scripted = (0..900usize)
        .map(|i| ControllerReport {
            seq: i as u32,
            buttons: 0,
            axes: [9.0, -9.0, 0.2, 0.75, 9.0, 0.0, 0.0, 0.0],
            timestamp_ns: i as u64 * 5_000_000,
        })
        .collect();

    let input = Arc::new(SpscRing::<ControlCommand>::with_capacity(64));
    let output = Arc::new(SpscRing::<ControlCommand>::with_capacity(64));
    let machine = Arc::new(StateMachine::new());
    machine.try_transition(Mode::Assist).unwrap();
    machine.try_transition(Mode::FullTeleop).unwrap();

    let stage = InputStage::new(
        ScriptedSource::new(scripted),
        ControllerMap::default(),
        Arc::clone(&input),
    );

    let mut l = SafetyLoop::new(
        input,
        Arc::clone(&output),
        machine,
        SafetyConfig {
            fence: GeoFence {
                radius_m: 60.0,
                max_alt_m: 20.0,
                ..GeoFence::default()
            },
            transition_s: 0.05,
            ..SafetyConfig::default()
        },
    );
    let _ = &stage.haptics;

    let mut imu = ImuSample::zeroed(0, 0);
    let mut gps = GpsSample::zeroed(0, 0);
    let mut pose = Pose6D::default();
    let mut stage = stage;

    let mut max_roll_out = 0.0f32;
    let ticks = 900usize;
    for tick in 0..ticks {
        let now = tick as u64 * 5_000_000;

        // Ingest: HID poll → map → ring (zero-copy cast into the command).
        let _ = stage.tick(now);
        // Intercept: safety loop sanitizes and forwards.
        l.process_one(now);
        // Flight controller consumes the sanitized command.
        if let Some(safe) = output.pop() {
            max_roll_out = max_roll_out.max(safe.axes[axis::ROLL].abs());
            can_op
                .send(&build_motor_frame(safe.axes[axis::THROTTLE]))
                .unwrap();
        }
        // Vehicle side.
        loop {
            let mut f = CanFrame::new(0, &[]);
            if !can_veh.recv(&mut f) {
                break;
            }
            drone.handle_can(&f);
        }
        drone.apply_actuation(&mut world, DT_S);
        world.step(DT_S);
        drone.post_step(&world, DT_S, &mut imu, &mut gps, &mut pose);
        l.set_pose(&pose);
    }

    // Envelope guarantees hold end-to-end despite the hostile operator.
    assert!(
        max_roll_out <= 0.35 + 1e-5,
        "tilt leak through safety loop: {max_roll_out}"
    );
    assert!(pose.z < 20.0, "ceiling breach: {}", pose.z);
    assert!(pose.x.hypot(pose.y) < 45.0, "fence radius breach");
    assert_eq!(stage_tick_probe(), ());
}

// Placeholder keeping the helper count stable across cfgs.
fn stage_tick_probe() {}
