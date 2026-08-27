//! The full-pipeline harness: Ingest → Normalize → Route → Safety →
//! Serialize → Transmit, with an optional physics-sim sink to prove the
//! sanitized command actually flies a vehicle inside its envelope.

use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tpt_t_core::bus::{MessageBus, SubscriberId};
use tpt_t_core::ser::{ControlCommand, GpsSample, ImuSample};
use tpt_t_core::{Mode, StateMachine};
use tpt_t_hal::mock::CanEndpoint;
use tpt_t_hal::{CanBus, CanFrame, Pose6D, QuadDrone, World, can_pair, ids, sim::DT_S};
use tpt_t_input::{ControllerMap, ControllerReport, DeviceInfo, InputStage, RawInputSource};
use tpt_t_link::mux::{Inbound, MAX_DATAGRAM, RxBuffer, UdpMux};
use tpt_t_ring::SpscRing;
use tpt_t_safety::{SafetyConfig, SafetyLoop, axis};

/// A `RawInputSource` fed from a shared lock-free ring so the harness can
/// script controller reports without touching the heap or taking a lock at
/// runtime. The harness owns the producer end; the source polls the consumer
/// end.
struct SharedScriptedSource {
    inner: Arc<SpscRing<ControllerReport>>,
    info: DeviceInfo,
}

impl SharedScriptedSource {
    fn new(inner: Arc<SpscRing<ControllerReport>>) -> Self {
        Self {
            inner,
            info: DeviceInfo {
                vendor_id: 0,
                product_id: 0,
                path: String::new(),
                num_axes: 8,
                num_buttons: 0,
            },
        }
    }
}

impl RawInputSource for SharedScriptedSource {
    fn poll(&mut self, out: &mut ControllerReport) -> bool {
        match self.inner.pop() {
            Some(r) => {
                *out = r;
                true
            }
            None => false,
        }
    }

    fn info(&self) -> &DeviceInfo {
        &self.info
    }

    fn reopen(&mut self) -> Result<(), tpt_t_input::InputError> {
        // Drain so a reopen starts clean; the ring is reused, no realloc.
        while self.inner.pop().is_some() {}
        Ok(())
    }
}

/// Physics-sim sink: consumes the safety-approved command, advances the
/// quadrotor one fixed step, and exposes the resulting pose.
pub struct SimState {
    world: World,
    drone: QuadDrone,
    can_op: CanEndpoint,
    can_veh: CanEndpoint,
    imu: ImuSample,
    gps: GpsSample,
    pose: Pose6D,
}

impl SimState {
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

    fn tick(&mut self, safe: &ControlCommand) {
        let thrust = safe.axes[axis::THROTTLE].clamp(0.0, 1.0);
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

/// Builds the `MOTOR_CMD` CAN frame a flight controller expects.
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

/// Wires the entire zero-copy data plane into one reusable object.
pub struct PipelineHarness {
    /// Ingest output (normalized commands) before routing.
    input: Arc<SpscRing<ControlCommand>>,
    /// Safety loop input (fed by the router).
    safety_in: Arc<SpscRing<ControlCommand>>,
    /// Safety loop output (sanitized commands).
    safety_out: Arc<SpscRing<ControlCommand>>,
    /// The deterministic intercept loop.
    loop_: SafetyLoop,
    /// Routing stage: fan-out to the safety subscriber + a logger subscriber.
    bus: MessageBus<ControlCommand>,
    safety_sub: SubscriberId,
    logger_sub: SubscriberId,
    /// Ingest stage (scripted HID source → map → `input` ring).
    ingest: InputStage<SharedScriptedSource>,
    /// Shared report ring the harness feeds reports into.
    queue: Arc<SpscRing<ControllerReport>>,
    /// Optional physics-sim sink.
    sim: Option<SimState>,
    /// Transmitting link mux.
    tx: UdpMux,
    /// Receiving link mux.
    rx: UdpMux,
    /// Destination address of `rx`.
    dst: SocketAddr,
    /// Reused transmit datagram buffer.
    send_buf: [u8; MAX_DATAGRAM],
    /// Reused receive scratch buffer.
    rx_buf: RxBuffer,
    /// Reused routing scratch (safety subscriber drain).
    route_scratch: Vec<ControlCommand>,
    /// Reused routing scratch (logger subscriber drain).
    logger_scratch: Vec<ControlCommand>,
    /// Count of commands delivered through the routing stage.
    routed_count: u64,
}

impl PipelineHarness {
    /// Builds with the default safety config and settles the mode machine into
    /// Full-Teleop authority (so the authority blend is a no-op on the axes).
    pub fn build() -> io::Result<Self> {
        Self::build_with(SafetyConfig::default())
    }

    /// Builds with a caller-supplied safety config, then settles authority.
    pub fn build_with(cfg: SafetyConfig) -> io::Result<Self> {
        let input = Arc::new(SpscRing::with_capacity(1024));
        let safety_in = Arc::new(SpscRing::with_capacity(1024));
        let safety_out = Arc::new(SpscRing::with_capacity(1024));
        let machine = Arc::new(StateMachine::new());

        let mut loop_ = SafetyLoop::new(
            Arc::clone(&safety_in),
            Arc::clone(&safety_out),
            Arc::clone(&machine),
            cfg,
        );

        // Auto → Assist → FullTeleop (a direct jump is disallowed by the
        // transition table), then idle ticks to drive the authority blend to 1.
        let _ = loop_.request_mode(Mode::Assist);
        let _ = loop_.request_mode(Mode::FullTeleop);
        for _ in 0..500 {
            loop_.process_one(0);
        }

        let mut bus: MessageBus<ControlCommand> = MessageBus::new(1024);
        let safety_sub = bus.subscribe();
        let logger_sub = bus.subscribe();

        let tx = UdpMux::bind_loopback()?;
        let rx = UdpMux::bind_loopback()?;
        let dst = rx.local_addr()?;

        let queue = Arc::new(SpscRing::with_capacity(256));
        let ingest = InputStage::new(
            SharedScriptedSource::new(Arc::clone(&queue)),
            ControllerMap::default(),
            Arc::clone(&input),
        );

        Ok(Self {
            input,
            safety_in,
            safety_out,
            loop_,
            bus,
            safety_sub,
            logger_sub,
            ingest,
            queue,
            sim: None,
            tx,
            rx,
            dst,
            send_buf: [0u8; MAX_DATAGRAM],
            rx_buf: RxBuffer::new(),
            route_scratch: Vec::with_capacity(64),
            logger_scratch: Vec::with_capacity(64),
            routed_count: 0,
        })
    }

    /// Enables the physics-sim sink so [`step`](Self::step) drives a vehicle.
    pub fn enable_sim(&mut self) {
        self.sim = Some(SimState::new());
    }

    /// Queues one synthetic controller report for the ingest stage.
    pub fn feed_report(&mut self, r: ControllerReport) {
        let _ = self.queue.push(r);
    }

    /// Runs one ingest poll (source → map → `input` ring). Exposed so
    /// allocation/hot-path audits can drive the ingest stage in isolation.
    pub fn ingest_tick(&mut self, now_ns: u64) {
        let _ = self.ingest.tick(now_ns);
    }

    /// Number of commands delivered through the routing stage so far.
    pub fn routed(&self) -> u64 {
        self.routed_count
    }

    /// Latest simulated pose (valid only after [`enable_sim`](Self::enable_sim)).
    pub fn pose(&self) -> Pose6D {
        self.sim.as_ref().map(|s| s.pose).unwrap_or_default()
    }

    /// Routing stage: drain the ingest output, fan it out via the lock-free
    /// bus, and feed the safety subscriber ring the loop consumes.
    pub fn route(&mut self) {
        while let Some(cmd) = self.input.pop() {
            self.bus.publish(cmd);
            self.routed_count += 1;
        }
        self.route_scratch.clear();
        self.bus.poll(self.safety_sub, &mut self.route_scratch);
        for c in self.route_scratch.drain(..) {
            let _ = self.safety_in.push(c);
        }
        // The logger subscriber demonstrates routing to a second downstream
        // consumer (recorder/FDR); we drop the copy after counting it.
        self.logger_scratch.clear();
        self.bus.poll(self.logger_sub, &mut self.logger_scratch);
    }

    /// Runs the hot tail: Safety → Serialize → Transmit → Receive/decode.
    fn run_hot(&mut self, now_ns: u64) -> io::Result<Option<ControlCommand>> {
        self.loop_.process_one(now_ns);
        let safe = self.safety_out.pop();
        if let Some(ref s) = safe {
            if let Some(ref mut sim) = self.sim {
                sim.tick(s);
                self.loop_.set_pose(&sim.pose);
            }
            let n = self.tx.write_control_frame(s, 0, &mut self.send_buf)?;
            self.tx.send_framed(self.dst, &self.send_buf[..n], now_ns)?;
        }
        self.recv_one()
    }

    /// Full pipeline with the ingest stage: feed a report via
    /// [`feed_report`](Self::feed_report) first, then call this each tick.
    pub fn step(&mut self, now_ns: u64) -> io::Result<Option<ControlCommand>> {
        let _ = self.ingest.tick(now_ns);
        self.route();
        self.run_hot(now_ns)
    }

    /// Full pipeline without the ingest stage: push a ready command and run
    /// Route → Safety → Serialize → Transmit → Receive.
    pub fn step_direct(
        &mut self,
        cmd: ControlCommand,
        now_ns: u64,
    ) -> io::Result<Option<ControlCommand>> {
        let _ = self.input.push(cmd);
        self.route();
        self.run_hot(now_ns)
    }

    /// Forward-only hot path (no blocking receive): Ingest → Normalize →
    /// Route → Safety → Serialize → Transmit. Returns bytes sent. This is the
    /// command-emitter's real-time path; the blocking UDP receive lives on the
    /// peer/consumer side and is out of scope for the zero-alloc budget.
    pub fn pump_forward(&mut self, now_ns: u64) -> io::Result<usize> {
        self.ingest_tick(now_ns);
        self.route();
        self.forward_tail(now_ns)
    }

    /// Forward-only path with a pre-built command (no ingest stage).
    pub fn pump_forward_direct(&mut self, cmd: ControlCommand, now_ns: u64) -> io::Result<usize> {
        let _ = self.input.push(cmd);
        self.route();
        self.forward_tail(now_ns)
    }

    /// Safety → Serialize → Transmit, returning bytes sent.
    fn forward_tail(&mut self, now_ns: u64) -> io::Result<usize> {
        self.loop_.process_one(now_ns);
        let safe = self.safety_out.pop();
        if let Some(ref s) = safe {
            if let Some(ref mut sim) = self.sim {
                sim.tick(s);
                self.loop_.set_pose(&sim.pose);
            }
            let n = self.tx.write_control_frame(s, 0, &mut self.send_buf)?;
            let sent = self.tx.send_framed(self.dst, &self.send_buf[..n], now_ns)?;
            Ok(sent)
        } else {
            Ok(0)
        }
    }

    /// Drains one datagram from the receive mux and returns the decoded
    /// command (zero-copy archived view copied to an owned struct).
    fn recv_one(&mut self) -> io::Result<Option<ControlCommand>> {
        for _ in 0..400 {
            match self.rx.recv_frame(&mut self.rx_buf)? {
                Some(Ok(inb)) => {
                    if let Inbound::Control { cmd, .. } = inb {
                        return Ok(Some(UdpMux::command_from_archived(cmd)));
                    }
                }
                Some(Err(_)) => {}
                None => std::thread::sleep(Duration::from_micros(20)),
            }
        }
        Ok(None)
    }
}
