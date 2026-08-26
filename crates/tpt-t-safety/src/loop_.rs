//! The deterministic safety intercept loop (spec §5.4).
//!
//! Topology per spec §6 "Safety" stage:
//!
//! ```text
//! input ring ──pop──▶ [pipeline] ──push──▶ output ring
//! ```
//!
//! Pipeline stages (each mutating the popped `ControlCommand` **in place**):
//! latency compensation → authority blend → geofence clamp → kinematic
//! limits → E-stop override (last, unconditional). The whole pipeline runs
//! lock-free and allocation-free — the <10 µs intercept budget in
//! `benches/intercept_bench.rs` measures exactly this section.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use tpt_t_core::ser::ControlCommand;
use tpt_t_core::{Mode, StateMachine};
use tpt_t_ring::SpscRing;

use crate::geo::{FenceVerdict, GeoFence};
use crate::latency::LatencyCompensator;
use crate::limits::{KinematicLimits, write_emergency_stop};
use crate::rt;
use crate::spline::{AuthorityBlend, authority_target};
use crate::veto::VetoGate;
use tpt_t_hal::Pose6D;

/// Tunables for one loop instance.
#[derive(Debug, Clone)]
pub struct SafetyConfig {
    /// Keep-in volume.
    pub fence: GeoFence,
    /// Kinematic envelope.
    pub limits: KinematicLimits,
    /// Spline duration for autonomy-handover blends.
    pub transition_s: f32,
    /// Loop tick period (µs); also the blend/slew time base.
    pub tick_period_us: u64,
    /// Delay at which attenuation bottoms out.
    pub max_latency_ns: u64,
}

impl Default for SafetyConfig {
    fn default() -> Self {
        Self {
            fence: GeoFence::default(),
            limits: KinematicLimits::default(),
            transition_s: 1.0,
            tick_period_us: 5_000,
            max_latency_ns: 50_000_000,
        }
    }
}

/// Per-intercept measurements.
#[derive(Debug, Clone, Copy)]
pub struct InterceptStats {
    /// Commands drained from the input ring this tick.
    pub drained: usize,
    /// Wall time spent inside the pure pipeline section (ns).
    pub intercepted_ns: u64,
}

/// The safety loop. See module docs for topology.
pub struct SafetyLoop {
    input: Arc<SpscRing<ControlCommand>>,
    output: Arc<SpscRing<ControlCommand>>,
    machine: Arc<StateMachine>,
    cfg: SafetyConfig,
    authority: f32,
    blend: Option<AuthorityBlend>,
    latency: LatencyCompensator,
    baseline: ControlCommand,
    last_out: ControlCommand,
    pose: Pose6D,
    estop_latched: bool,
    dropped_out: u64,
    veto: VetoGate,
    prev_z: f64,
    have_prev_z: bool,
    vz_est: f64,
}

// SAFETY: all shared state is atomic/ring-mediated; the pose snapshot is
// touched only by the owning thread through &mut self methods.
unsafe impl Send for SafetyLoop {}

impl SafetyLoop {
    /// Wires the loop between an input ring and an output ring, bound to the
    /// central [`StateMachine`].
    pub fn new(
        input: Arc<SpscRing<ControlCommand>>,
        output: Arc<SpscRing<ControlCommand>>,
        machine: Arc<StateMachine>,
        cfg: SafetyConfig,
    ) -> Self {
        let current = machine.current();
        Self {
            input,
            output,
            machine,
            authority: crate::spline::authority_target(current),
            blend: None,
            latency: LatencyCompensator::new(0.2, cfg.max_latency_ns),
            baseline: ControlCommand::zeroed(Mode::Auto),
            last_out: ControlCommand::zeroed(current),
            pose: Pose6D::default(),
            estop_latched: false,
            dropped_out: 0,
            prev_z: 0.0,
            have_prev_z: false,
            vz_est: 0.0,
            veto: VetoGate::default(),
            cfg,
        }
    }

    /// Feeds the freshest vehicle pose (odometry/GPS fusion). Called once per
    /// tick, this also maintains the vertical-speed estimate used by the
    /// predictive ceiling guard.
    #[inline]
    pub fn set_pose(&mut self, pose: &Pose6D) {
        if self.have_prev_z {
            let dt_s = self.cfg.tick_period_us as f64 / 1e6;
            if dt_s > 0.0 {
                self.vz_est = (pose.z - self.prev_z) / dt_s;
            }
        }
        self.prev_z = pose.z;
        self.have_prev_z = true;
        self.pose = *pose;
    }

    /// Publishes the latest autonomy-produced command (the Auto-side input of
    /// the authority mix). Call at whatever cadence autonomy runs.
    pub fn set_autonomy_baseline(&mut self, cmd: &ControlCommand) {
        self.baseline = *cmd;
    }

    /// Requests a mode change; starts a spline blend toward the matching
    /// authority target. Illegal transitions are refused unchanged.
    pub fn request_mode(&mut self, mode: Mode) -> Result<(), tpt_t_core::ModeError> {
        self.machine.try_transition(mode)?;
        let to = authority_target(mode);
        if (to - self.authority).abs() > 1e-6 {
            self.blend = Some(AuthorityBlend::new(
                self.authority,
                to,
                self.cfg.transition_s,
            ));
        }
        Ok(())
    }

    /// Latches the emergency stop. Every subsequently processed command is
    /// the zeroed safe state; the latch is sticky by design (restart requires
    /// explicit acknowledgment upstream).
    pub fn emergency_stop(&mut self) {
        self.estop_latched = true;
        self.machine.force_emergency_stop();
    }

    /// E-stop latch state.
    #[inline]
    pub fn estop_latched(&self) -> bool {
        self.estop_latched
    }

    /// Engages the shared-control AI veto (spec §5.4): while active, the
    /// human's translational commands are clamped to the AI-derived cap.
    /// The AI side can only restrict — [`VetoGate`] is downward-only and can
    /// never inject new intent.
    pub fn engage_ai_veto(&self, max_translational_norm: f32) {
        self.veto.engage(max_translational_norm);
    }

    /// Disengages the AI veto.
    pub fn disengage_ai_veto(&self) {
        self.veto.disengage();
    }

    /// Output-ring drops observed so far.
    #[inline]
    pub fn dropped_outputs(&self) -> u64 {
        self.dropped_out
    }

    /// Current authority weight (1 = teleop, 0 = autonomy).
    #[inline]
    pub fn authority(&self) -> f32 {
        self.authority
    }
}

impl SafetyLoop {
    /// Drains the input ring keeping the newest command and runs the full
    /// pipeline on it in place, pushing the result.
    ///
    /// Time-based state (mode-blend animation) advances on **every** tick,
    /// idle or not — `stats.drained == 0` marks an idle tick.
    pub fn process_one(&mut self, now_ns: u64) -> InterceptStats {
        let t0 = Instant::now();

        // 0) Blend progresses with wall-clock ticks, command or not.
        if let Some(blend) = &mut self.blend {
            self.authority = blend.advance(self.cfg.tick_period_us as f32 / 1e6);
        }

        let mut newest: Option<ControlCommand> = None;
        let mut drained = 0usize;
        while let Some(c) = self.input.pop() {
            newest = Some(c);
            drained += 1;
        }
        let mut cmd = match newest {
            Some(c) => c,
            None => {
                return InterceptStats {
                    drained: 0,
                    intercepted_ns: t0.elapsed().as_nanos() as u64,
                };
            }
        };

        // 1) Latency compensation on the raw operator input.
        self.latency.observe(now_ns, cmd.timestamp_ns);
        self.latency.compensate(&mut cmd);

        // 2) Authority blend (autonomy ↔ operator), C²-smooth.
        if let Some(blend) = &mut self.blend {
            self.authority = blend.advance(self.cfg.tick_period_us as f32 / 1e6);
        }
        if self.authority < 0.9999 {
            for k in 0..cmd.axes.len() {
                cmd.axes[k] =
                    self.baseline.axes[k] * (1.0 - self.authority) + cmd.axes[k] * self.authority;
            }
            cmd.mode = if self.authority >= 0.5 {
                cmd.mode
            } else {
                self.baseline.mode
            };
        }

        // 2b) Shared-control AI veto: restrict-only, never injects intent.
        let _ = self.veto.apply(&mut cmd);

        // 2c) Predictive vertical brake: start shedding climb *before* the
        // ceiling, using braking distance v²/(2a) from the measured climb
        // rate (spec §5.4 predictive avoidance). Hover-neutral throttle
        // yields ~2 m/s² of downward authority on typical frames.
        const CEILING_BRAKE_M_S2: f64 = 2.0;
        let stop_dist = self.vz_est * self.vz_est / (2.0 * CEILING_BRAKE_M_S2);
        if self.pose.z + stop_dist >= self.cfg.fence.max_alt_m
            && cmd.axes[3] > crate::geo::THROTTLE_NEUTRAL
        {
            cmd.axes[3] = crate::geo::THROTTLE_NEUTRAL;
        }
        // 3) Geofence projection (soft attenuate / hard suppress).
        let verdict = self.cfg.fence.evaluate(&self.pose);
        if verdict != FenceVerdict::Inside {
            self.cfg.fence.clamp_command(&self.pose, verdict, &mut cmd);
            if verdict == FenceVerdict::HardBreach && !self.estop_latched {
                self.estop_latched = true;
                self.machine.force_emergency_stop();
            }
        }

        // 4) Kinematic limits: absolute clamps + slew vs last accepted.
        self.cfg.limits.apply(&self.last_out, &mut cmd);

        // 5) Emergency override — always last, always wins.
        if self.estop_latched {
            write_emergency_stop(cmd.seq, now_ns, &mut cmd);
        }

        self.last_out = cmd;
        let intercepted_ns = t0.elapsed().as_nanos() as u64;

        if self.output.push(cmd).is_err() {
            // Downstream stalled: count the drop, never block the RT thread.
            self.dropped_out += 1;
        }
        InterceptStats {
            drained,
            intercepted_ns,
        }
    }

    /// Runs ticks until `stop` is raised. Sleep-based pacing keeps CPU usage
    /// sane; the intercept section itself is measured separately by
    /// [`process_one`](Self::process_one).
    pub fn run_until(&mut self, stop: &AtomicBool) {
        let period = Duration::from_micros(self.cfg.tick_period_us);
        while !stop.load(Ordering::Relaxed) {
            let _ = self.process_one(unix_ns_now());
            std::thread::sleep(period);
        }
    }

    /// Spawns the loop on a named background thread, attempting RT elevation
    /// first. The outcome is exposed via
    /// [`SafetyThreadHandle::rt_elevated`](SafetyThreadHandle::rt_elevated).
    pub fn spawn(mut self) -> std::io::Result<SafetyThreadHandle> {
        let stop = Arc::new(AtomicBool::new(false));
        let rt_ok = Arc::new(AtomicBool::new(false));
        let stop_t = Arc::clone(&stop);
        let rt_t = Arc::clone(&rt_ok);
        let period = Duration::from_micros(self.cfg.tick_period_us);
        let join = std::thread::Builder::new()
            .name("tpt:safety".into())
            .spawn(move || {
                rt_t.store(rt::elevate_current_thread().is_ok(), Ordering::Release);
                while !stop_t.load(Ordering::Relaxed) {
                    let _ = self.process_one(unix_ns_now());
                    std::thread::sleep(period);
                }
            })?;
        Ok(SafetyThreadHandle {
            stop,
            rt_ok,
            join: Some(join),
        })
    }
}

/// Joinable handle to a spawned safety thread.
pub struct SafetyThreadHandle {
    stop: Arc<AtomicBool>,
    rt_ok: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl SafetyThreadHandle {
    /// Whether RT priority elevation succeeded on the loop thread.
    pub fn rt_elevated(&self) -> bool {
        self.rt_ok.load(Ordering::Acquire)
    }

    /// Signals shutdown and joins the thread.
    pub fn stop(mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

#[inline]
fn unix_ns_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}
