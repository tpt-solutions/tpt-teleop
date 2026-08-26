//! Network service: drives [`UdpMux`] from the Phase 2 platform event loop
//! (epoll / kqueue / IOCP) on one pinned thread — no async runtime anywhere
//! (spec §3.1, §5.2).
//!
//! Responsibilities per [`poll`](NetService::poll) tick:
//!
//! 1. Dispatch the platform loop; on socket readability, drain datagrams
//!    until the kernel buffer is empty (edge-triggered semantics demand it)
//!    and hand each demultiplexed frame to the caller's sink as an owned
//!    [`Event`].
//! 2. Drain the safety output ring (`tpt-t-safety` pushes approved
//!    commands; this is step "Serialize" of the §6 pipeline) and transmit.
//! 3. Emit mesh beacons on schedule, expire silent neighbors.
//! 4. Drive the reliable channel (acks out, RTO resends out, RTT samples
//!    into backpressure).
//!
//! Windows note: IOCP reports nothing for a non-overlapped socket, so the
//! loop relies on the bounded dispatch timeout as its heartbeat — identical
//! code paths work on every backend because readiness is advisory here.

use core::sync::atomic::Ordering;
use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tpt_t_core::eventloop::{EventHandler, EventLoop, PlatformLoop, Ready, Target};
use tpt_t_core::ser::{ControlCommand, TelemetryPacket};
use tpt_t_ring::SpscRing;

use crate::mesh::{MeshBeacon, NeighborTable};
use crate::mux::{HEADER_LEN, Inbound, MAX_DATAGRAM, RxBuffer, TRAILER_LEN, UdpMux, flags};
use crate::reliable::{ReliableConfig, ReliableRx, ReliableTx};

/// Registration token for the mux socket inside the platform loop.
pub const TOKEN_SOCKET: u64 = 1;

/// Service configuration.
#[derive(Debug, Clone)]
pub struct ServiceConfig {
    /// Stable unit identity used in mesh beacons.
    pub unit_id: u64,
    /// Port to bind (`0` = ephemeral; production units use 443).
    pub listen_port: u16,
    /// Beacon period.
    pub beacon_interval_ns: u64,
    /// Neighbor time-to-live.
    pub mesh_ttl_ns: u64,
    /// Bootstrap addresses that receive beacons (unicast; swarm membership
    /// propagates because every new neighbor is answered immediately).
    pub beacon_peers: Vec<SocketAddr>,
    /// Fallback destination for acks/retransmits when traffic has not yet
    /// revealed a peer address. Multi-peer session routing lands with the
    /// cloud layer (spec §5.6).
    pub primary_peer: Option<SocketAddr>,
    /// Route control commands through the reliable channel.
    pub reliable_control: bool,
    /// Retransmission timeout for the reliable channel.
    pub rto_ns: u64,
}

impl Default for ServiceConfig {
    fn default() -> Self {
        Self {
            unit_id: 0,
            listen_port: 0,
            beacon_interval_ns: 200_000_000,
            mesh_ttl_ns: 5_000_000_000,
            beacon_peers: Vec::new(),
            primary_peer: None,
            reliable_control: false,
            rto_ns: 10_000_000,
        }
    }
}

/// An owned event handed to the sink. Hot-path variants are fixed-size
/// copies; only ICE passthrough allocates (opaque external data — the WebRTC
/// stack owns fragmentation).
pub enum Event {
    /// Safety-approved operator command (CRC already verified).
    Control {
        /// Decoded command.
        cmd: ControlCommand,
        /// Sender.
        from: SocketAddr,
        /// Sent through the reliable channel (ack was or will be sent).
        reliable: bool,
    },
    /// Telemetry packet.
    Telemetry(TelemetryPacket, SocketAddr),
    /// Opaque ICE/STUN/DTLS bytes for the WebRTC stack.
    Ice(Vec<u8>, SocketAddr),
    /// A neighbor appeared or advanced its beacon sequence.
    NeighborUp(u64, SocketAddr),
}

impl core::fmt::Debug for Event {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Event::Control {
                cmd,
                from,
                reliable,
            } => f
                .debug_struct("Control")
                .field("seq", &cmd.seq)
                .field("from", from)
                .field("reliable", reliable)
                .finish(),
            Event::Telemetry(p, from) => f
                .debug_struct("Telemetry")
                .field("seq", &p.seq)
                .field("from", from)
                .finish(),
            Event::Ice(b, from) => f
                .debug_struct("Ice")
                .field("len", &b.len())
                .field("from", from)
                .finish(),
            Event::NeighborUp(id, addr) => {
                f.debug_tuple("NeighborUp").field(id).field(addr).finish()
            }
        }
    }
}

/// The mux + protocol state driven by one event loop.
/// Owns the platform event loop and all link state. `!Sync` by design: it
/// lives on the network thread; other threads reach it through `outbox`
/// (pushing approved commands) and the atomics inside [`UdpMux`].
pub struct NetService {
    ev: PlatformLoop,
    core: ServiceCore,
}

impl NetService {
    /// Binds a mux and registers it with this platform's event loop.
    ///
    /// `safety_out` is the ring the safety loop pushes approved commands
    /// into — the "Serialize" step of spec §6 reads from here every tick.
    pub fn new(cfg: ServiceConfig, safety_out: Arc<SpscRing<ControlCommand>>) -> io::Result<Self> {
        let mux = if cfg.listen_port == 0 {
            UdpMux::bind_loopback()?
        } else {
            UdpMux::bind(cfg.listen_port)?
        };
        let mut ev = PlatformLoop::new()?;
        // SAFETY-free: Target construction is platform-conditional but safe.
        #[cfg(unix)]
        let target = Target::Fd(mux.as_raw_fd());
        #[cfg(windows)]
        let target = Target::Handle(mux.as_raw_handle_value());
        ev.register(target, TOKEN_SOCKET, Ready::READ | Ready::WRITE)?;
        #[allow(unused_mut)]
        let mut core = ServiceCore {
            mux,
            rx: Box::new(RxBuffer::new()),
            tx: Box::new([0u8; MAX_DATAGRAM]),
            cfg,
            mesh: NeighborTable::default(),
            rel_tx: ReliableTx::new(ReliableConfig::default()),
            rel_rx: ReliableRx::new(),
            outbox: safety_out,
            anchor: std::time::Instant::now(),
            last_beacon_ns: 0,
            beacon_seq: 0,
            #[cfg(target_os = "linux")]
            uring: None,
        };
        // Attach an io_uring transmit queue when the kernel supports it; a
        // setup failure (old kernel / seccomp-restricted container) leaves the
        // portable socket path in place.
        #[cfg(target_os = "linux")]
        {
            core.uring =
                crate::uring::UringTx::new(core.mux.as_raw_fd(), crate::uring::DEFAULT_SLOTS).ok();
        }
        Ok(Self { ev, core })
    }

    /// Local bound address (peers learn the port via beacons too).
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.core.mux.local_addr()
    }

    /// Neighbor table snapshot hook for dashboards/tests.
    pub fn neighbors(&self) -> impl Iterator<Item = crate::mesh::NeighborEntry> + '_ {
        self.core.mesh.iter()
    }

    /// Whether the io_uring transmit path is active (Linux only).
    pub fn using_uring(&self) -> bool {
        self.core.using_uring()
    }

    /// Smoothed reliable-channel RTT estimate feeding backpressure.
    pub fn rtt_estimate_ns(&self) -> u64 {
        self.core.mux.backpressure.rtt_estimate_ns()
    }

    /// Inbound CRC rejections seen so far.
    pub fn crc_errors(&self) -> u64 {
        self.core.mux.stats.rx_crc_errors.load(Ordering::Relaxed)
    }

    /// Test/ops escape hatch: send an arbitrary datagram verbatim.
    pub fn raw_send(&mut self, dst: SocketAddr, bytes: &[u8]) -> io::Result<usize> {
        let now = self.core.now_ns();
        self.core.mux.send_framed(dst, bytes, now)
    }

    /// Test hook: inbound frames demultiplexed so far.
    pub fn core_rx_frames(&self) -> u64 {
        self.core.mux.stats.rx_frames.load(Ordering::Relaxed)
    }

    /// Test hook: inbound size/magic rejects so far.
    pub fn core_malformed(&self) -> u64 {
        self.core.mux.stats.rx_malformed.load(Ordering::Relaxed)
    }

    /// Test hook: outbound frames transmitted so far.
    pub fn core_tx_frames(&self) -> u64 {
        self.core.mux.stats.tx_frames.load(Ordering::Relaxed)
    }

    /// Runs one service tick: platform dispatch + drain + outbox flush +
    /// beacon/retransmit timers. Events are delivered to `sink`, which also
    /// gets [`&mut ServiceCore`](ServiceCore) so it can reply on the spot.
    pub fn poll(
        &mut self,
        timeout: Duration,
        sink: &mut dyn FnMut(Event, &mut ServiceCore),
    ) -> io::Result<usize> {
        // Split borrows: the session takes everything except the loop.
        let NetService { ev, core } = self;
        let mut session = Session {
            core,
            sink,
            handled: 0,
        };
        let dispatched = ev.dispatch(Some(timeout), &mut session)?;
        // Heartbeat drain — see `Session::drain_now` for why this is
        // unconditional (IOCP + non-overlapped sockets never fire READ).
        session.drain_now();
        let handled = session.handled;
        self.background();
        Ok(dispatched.max(handled))
    }

    /// Timers that run regardless of socket readiness.
    fn background(&mut self) {
        let now = self.core.now_ns();

        // Safety-output ring → wire ("Serialize" step of spec §6).
        while let Some(cmd) = self.core.outbox.pop() {
            match self.core.primary_dst() {
                Some(dst) => {
                    if self.core.send_control(&cmd, dst).is_err() {
                        break; // kernel buffer full; retry next tick
                    }
                }
                None => break, // nobody to talk to yet
            }
        }

        // Mesh beacon heartbeat.
        if now.saturating_sub(self.core.last_beacon_ns) >= self.core.cfg.beacon_interval_ns {
            self.core.beacon_seq = self.core.beacon_seq.wrapping_add(1);
            self.core.last_beacon_ns = now;
            self.core.beacon_fanout();
            self.core.mesh.expire(now, self.core.cfg.mesh_ttl_ns);
        }

        // Reliable-channel RTO resends toward the primary peer.
        if let Some(dst) = self.core.primary_dst() {
            let rto = self.core.cfg.rto_ns;
            // Split the borrow: tick owns rel_tx; the closure touches mux.
            let core = &mut self.core;
            let ServiceCore {
                rel_tx,
                mux,
                anchor,
                ..
            } = core;
            let now = anchor.elapsed().as_nanos() as u64;
            rel_tx.tick(now, rto, &mut |_seq, framed| {
                let _ = mux.send_framed(dst, framed, now);
            });
        }
    }
}

/// The mux + protocol state driven by one event loop. Public so the sink
/// callback can reply immediately (send commands/telemetry/ICE, fan out
/// beacons, inspect neighbors and link counters).
pub struct ServiceCore {
    mux: UdpMux,
    rx: Box<RxBuffer>,
    tx: Box<[u8; MAX_DATAGRAM]>,
    cfg: ServiceConfig,
    mesh: NeighborTable,
    rel_tx: ReliableTx,
    rel_rx: ReliableRx,
    outbox: Arc<SpscRing<ControlCommand>>,
    anchor: std::time::Instant,
    last_beacon_ns: u64,
    beacon_seq: u32,
    /// Linux-only io_uring transmit queue (None when the kernel lacks
    /// io_uring support — the portable socket path is used instead).
    #[cfg(target_os = "linux")]
    uring: Option<crate::uring::UringTx>,
}

impl ServiceCore {
    fn now_ns(&self) -> u64 {
        self.anchor.elapsed().as_nanos() as u64
    }

    /// Transmits one already-framed datagram. On Linux with an io_uring queue
    /// attached, the bytes are copied into a submission-owned slot and handed
    /// straight to the kernel (the bounded slot count is itself the
    /// backpressure signal); when io_uring is unavailable the portable socket
    /// send is used. Returns the byte count on success.
    fn transmit(&mut self, dst: SocketAddr, buf: &[u8], now_ns: u64) -> io::Result<usize> {
        #[cfg(target_os = "linux")]
        {
            if let Some(uring) = &mut self.uring {
                if uring
                    .stage(dst, |slot| {
                        let n = buf.len().min(slot.len());
                        slot[..n].copy_from_slice(&buf[..n]);
                        n
                    })
                    .is_ok()
                {
                    let _ = uring.submit_staged();
                    uring.reap(|_| {});
                    crate::mux::LinkStats::bump(&self.mux.stats.tx_frames);
                    return Ok(buf.len());
                }
                // Slot exhaustion (or any stage failure) falls through to the
                // socket path, which applies its own backpressure accounting.
            }
        }
        self.mux.send_framed(dst, buf, now_ns)
    }

    /// Best current destination: explicit primary peer, else first known
    /// neighbor (mesh discovery may have found one).
    pub(crate) fn primary_dst(&self) -> Option<SocketAddr> {
        self.cfg
            .primary_peer
            .or_else(|| self.mesh.iter().next().map(|n| n.addr))
    }

    /// Link counters (frames sent/received, rejects, CRC errors…).
    pub fn stats(&self) -> &crate::mux::LinkStats {
        &self.mux.stats
    }

    /// Backpressure signal — Phase 8's encoder polls this for bitrate.
    pub fn backpressure(&self) -> &crate::backpressure::Backpressure {
        &self.mux.backpressure
    }

    /// Live neighbors (probe order).
    pub fn neighbors(&self) -> impl Iterator<Item = crate::mesh::NeighborEntry> + '_ {
        self.mesh.iter()
    }

    /// True when datagrams are leaving through the io_uring transmit path
    /// (Linux with kernel support); false on other platforms or when the
    /// portable socket send is in use.
    #[cfg(target_os = "linux")]
    pub fn using_uring(&self) -> bool {
        self.uring.is_some()
    }
    /// See [`ServiceCore::using_uring`].
    #[cfg(not(target_os = "linux"))]
    pub fn using_uring(&self) -> bool {
        false
    }

    /// Transmits one control command (reliable or fire-and-forget).
    pub fn send_control(&mut self, cmd: &ControlCommand, dst: SocketAddr) -> io::Result<()> {
        use crate::reliable::MAX_RELIABLE_PAYLOAD;
        let now = self.now_ns();
        // Reliable mode retains the whole framed datagram; commands are tiny
        // so this holds by construction (asserted in debug builds).
        debug_assert!(
            !self.cfg.reliable_control
                || HEADER_LEN + core::mem::size_of::<ControlCommand>() + TRAILER_LEN
                    <= MAX_RELIABLE_PAYLOAD,
            "framed control exceeds retention slot"
        );
        let frame_flags = if self.cfg.reliable_control {
            flags::RELIABLE
        } else {
            0
        };
        let n = self
            .mux
            .write_control_frame(cmd, frame_flags, &mut self.tx)?;
        if self.cfg.reliable_control && n <= MAX_RELIABLE_PAYLOAD {
            let mut retained = [0u8; MAX_RELIABLE_PAYLOAD];
            retained[..n].copy_from_slice(&self.tx[..n]);
            // The reliable channel tracks a 32-bit sequence window; the
            // command's 64-bit seq truncates wrap-safely.
            let _ = self.rel_tx.send(cmd.seq as u32, &retained[..n], now);
        }
        let mut frame = [0u8; MAX_DATAGRAM];
        frame[..n].copy_from_slice(&self.tx[..n]);
        self.transmit(dst, &frame[..n], now).map(|_| ())
    }

    /// Transmits one telemetry packet (subject to bandwidth admission).
    pub fn send_telemetry(&mut self, pkt: &TelemetryPacket, dst: SocketAddr) -> io::Result<()> {
        let now = self.now_ns();
        let n = self.mux.write_telemetry_frame(pkt, &mut self.tx)?;
        // Telemetry yields to control under congestion.
        if !self.mux.backpressure.admit(n, now) {
            return Ok(());
        }
        let mut frame = [0u8; MAX_DATAGRAM];
        frame[..n].copy_from_slice(&self.tx[..n]);
        self.transmit(dst, &frame[..n], now).map(|_| ())
    }

    /// Forwards opaque ICE bytes toward `dst`.
    pub fn send_ice(&mut self, bytes: &[u8], dst: SocketAddr) -> io::Result<()> {
        let now = self.now_ns();
        let n = self.mux.write_ice_frame(bytes, &mut self.tx)?;
        let mut frame = [0u8; MAX_DATAGRAM];
        frame[..n].copy_from_slice(&self.tx[..n]);
        self.transmit(dst, &frame[..n], now).map(|_| ())
    }

    /// Emits one beacon round to every configured peer plus all known
    /// neighbors (swarm membership propagates from any seed).
    pub fn beacon_fanout(&mut self) {
        let peers: Vec<SocketAddr> = self
            .cfg
            .beacon_peers
            .iter()
            .copied()
            .chain(self.mesh.iter().map(|n| n.addr))
            .collect();
        for dst in peers {
            let _ = self.send_beacon_to(dst);
        }
    }

    fn send_beacon_to(&mut self, dst: SocketAddr) -> io::Result<()> {
        let now = self.now_ns();
        let port = self
            .mux
            .local_addr()
            .map(|a| a.port())
            .unwrap_or(self.cfg.listen_port);
        let beacon = MeshBeacon::new(self.cfg.unit_id, self.beacon_seq, now, port);
        let n = self.mux.write_mesh_frame(&beacon, &mut self.tx)?;
        let mut frame = [0u8; MAX_DATAGRAM];
        frame[..n].copy_from_slice(&self.tx[..n]);
        self.transmit(dst, &frame[..n], now).map(|_| ())
    }
}

/// Per-dispatch borrow bundle: everything the socket handler needs except
/// the event loop itself.
struct Session<'a> {
    core: &'a mut ServiceCore,
    sink: &'a mut dyn FnMut(Event, &mut ServiceCore),
    handled: usize,
}

impl EventHandler for Session<'_> {
    fn ready(&mut self, token: u64, ready: Ready) {
        if token != TOKEN_SOCKET || !ready.intersects(Ready::READ) {
            return;
        }
        self.drain_now();
    }
}

impl Session<'_> {
    /// Drains every pending datagram. Called from [`ready`](EventHandler::ready)
    /// on readability *and* unconditionally once per tick from
    /// [`NetService::poll`] — IOCP reports no completion for non-overlapped
    /// sockets, so Windows relies on the tick heartbeat while epoll/kqueue
    /// keep their event-driven fast path. An extra empty-socket probe costs
    /// one `WouldBlock` syscall.
    fn drain_now(&mut self) {
        // Edge-triggered contract: drain until the kernel buffer is empty.
        // Phased per frame so borrows never overlap:
        //   A) recv+demux touches `rx` (+mux internally),
        //   B) conversion to owned ends that borrow,
        //   C) protocol state + sink get the whole `core`.
        loop {
            use crate::mux::FrameError;

            // --- Phase A/B: one datagram in, owned frame out -------------
            enum Frame {
                Cmd(ControlCommand, SocketAddr, bool),
                Tlm(TelemetryPacket, SocketAddr),
                Ice(Vec<u8>, SocketAddr),
                Beacon(MeshBeacon, SocketAddr),
                Ack(crate::reliable::AckFrame),
            }
            let frame = match self.core.mux.recv_frame(&mut self.core.rx) {
                Ok(Some(Ok(inbound))) => match inbound {
                    Inbound::Control {
                        cmd,
                        from,
                        reliable,
                    } => Frame::Cmd(UdpMux::command_from_archived(cmd), from, reliable),
                    Inbound::Telemetry { pkt, from } => Frame::Tlm(
                        TelemetryPacket {
                            magic: pkt.magic.into(),
                            kind: pkt.kind.into(),
                            reserved: pkt.reserved.into(),
                            seq: pkt.seq.into(),
                            timestamp_ns: pkt.timestamp_ns.into(),
                            values: pkt.values.map(|v| v.into()),
                        },
                        from,
                    ),
                    Inbound::Ice { payload, from } => Frame::Ice(payload.to_vec(), from),
                    Inbound::Mesh { beacon, from } => Frame::Beacon(beacon, from),
                    Inbound::Ack { ack, .. } => Frame::Ack(ack),
                },
                Ok(Some(Err(e))) => {
                    let stats = &self.core.mux.stats;
                    match e {
                        FrameError::Malformed => crate::mux::LinkStats::bump(&stats.rx_malformed),
                        FrameError::Crc => crate::mux::LinkStats::bump(&stats.rx_crc_errors),
                        FrameError::Payload => crate::mux::LinkStats::bump(&stats.rx_rejected),
                    }
                    continue;
                }
                Ok(None) => break,
                Err(_) => break, // transient OS error; retry next tick
            };

            // --- Phase C: protocol state + upstream delivery -------------
            let now = self.core.now_ns();
            let event = match frame {
                Frame::Cmd(cmd, from, reliable) => {
                    if reliable {
                        // Track receive window + answer with a SACK.
                        let _ = self.core.rel_rx.accept(cmd.seq as u32, &[]);
                        let ack = self.core.rel_rx.build_ack(0);
                        if let Ok(n) = self.core.mux.write_ack_frame(&ack, &mut self.core.tx) {
                            let mut frame = [0u8; MAX_DATAGRAM];
                            frame[..n].copy_from_slice(&self.core.tx[..n]);
                            let _ = self.core.transmit(from, &frame[..n], now);
                        }
                    }
                    Some(Event::Control {
                        cmd,
                        from,
                        reliable,
                    })
                }
                Frame::Tlm(pkt, from) => Some(Event::Telemetry(pkt, from)),
                Frame::Ice(bytes, from) => Some(Event::Ice(bytes, from)),
                Frame::Beacon(beacon, src) => {
                    // Reply address comes from the beacon payload
                    // (authoritative listen port), not the datagram source.
                    let mut peer = src;
                    peer.set_port(beacon.listen_port);
                    if self
                        .core
                        .mesh
                        .observe(beacon.unit_id, peer, beacon.seq, now)
                    {
                        // Answer once so the newcomer learns us without
                        // waiting for its next heartbeat round.
                        let _ = self.core.send_beacon_to(peer);
                        Some(Event::NeighborUp(beacon.unit_id, peer))
                    } else {
                        None
                    }
                }
                Frame::Ack(ack) => {
                    if let Some(rtt) = self.core.rel_tx.on_ack(&ack, now) {
                        self.core.mux.backpressure.note_rtt_ns(rtt);
                    }
                    None
                }
            };
            if let Some(ev) = event {
                self.handled += 1;
                (self.sink)(ev, self.core);
            }
        }
    }
}
