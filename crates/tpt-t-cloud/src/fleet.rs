//! Multi-unit fleet orchestration (spec §5.6).
//!
//! One [`UnitSession`](UnitState) is kept per connected unit — the "many
//! concurrent DTI sessions" requirement. Each session owns its own SFU fan-out
//! ([`crate::sfu`]), its own session recorder ([`crate::recorder`]), and the
//! cloud-side view of the unit's autonomy mode. Commands to units (including
//! MCP-driven autonomy handovers) are sent through a pluggable
//! [`UnitTransport`]; the production implementation reuses the Phase 7
//! [`tpt_t_link`] UDP multiplexer ([`UdpTransport`]).

use std::collections::HashMap;
use std::io;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use tpt_t_core::mode::{Mode, TransitionTable};
use tpt_t_core::ser::ControlCommand;
use tpt_t_ring::SpscRing;

use crate::error::{CloudError, transport_err};
use crate::json::Json;
use crate::recorder::{FileRecorder, NullRecorder, Recorder};
use crate::sfu::{MediaFrame, SfuFanout, SubscriberId};

/// Sink for commands the cloud sends down to a unit.
pub trait UnitTransport: Send + std::any::Any {
    /// Sends `cmd` to `addr`. Failures are surfaced to the caller so policy
    /// (e.g. a dead peer) can be applied.
    fn send_command(&mut self, addr: SocketAddr, cmd: &ControlCommand) -> io::Result<()>;
}

/// A transport that drops every command (standalone tests / dry runs).
#[derive(Debug, Default, Clone)]
pub struct NullTransport;

impl UnitTransport for NullTransport {
    fn send_command(&mut self, _addr: SocketAddr, _cmd: &ControlCommand) -> io::Result<()> {
        Ok(())
    }
}

/// A transport that records every command it is asked to send (test assert).
#[derive(Debug, Default)]
pub struct CapturingTransport {
    /// Captured `(destination, command)` pairs in send order.
    pub sent: Vec<(SocketAddr, ControlCommand)>,
}

impl CapturingTransport {
    /// An empty capturing transport.
    pub fn new() -> Self {
        Self { sent: Vec::new() }
    }
}

impl UnitTransport for CapturingTransport {
    fn send_command(&mut self, addr: SocketAddr, cmd: &ControlCommand) -> io::Result<()> {
        self.sent.push((addr, *cmd));
        Ok(())
    }
}

/// UDP transport reusing the Phase 7 multiplexer to frame + CRC commands.
pub struct UdpTransport {
    mux: tpt_t_link::mux::UdpMux,
    scratch: Box<[u8; tpt_t_link::mux::MAX_DATAGRAM]>,
}

impl UdpTransport {
    /// Binds an ephemeral UDP socket for outbound commands to units.
    pub fn bind() -> io::Result<Self> {
        Ok(Self {
            mux: tpt_t_link::mux::UdpMux::bind_loopback()?,
            scratch: Box::new([0u8; tpt_t_link::mux::MAX_DATAGRAM]),
        })
    }
}

impl UnitTransport for UdpTransport {
    fn send_command(&mut self, addr: SocketAddr, cmd: &ControlCommand) -> io::Result<()> {
        let n = self
            .mux
            .write_control_frame(cmd, 0, &mut self.scratch)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
        let now = crate::now_unix_ns();
        self.mux
            .send_framed(addr, &self.scratch[..n], now)
            .map(|_| ())
    }
}

/// Live per-unit session state held by the fleet.
pub struct UnitState {
    /// Stable unit identity.
    pub id: u64,
    /// Last known wire address.
    pub addr: SocketAddr,
    /// Cloud-side view of the unit's autonomy mode.
    pub mode: Mode,
    /// Operator currently assigned (if any).
    pub assigned_operator: Option<String>,
    /// Last ingested frame sequence.
    pub last_seq: u64,
    /// Per-unit monotonic command sequence number.
    pub cmd_seq: u64,
    /// Session creation timestamp (UNIX ns).
    pub created_ns: u64,
    /// Last inbound frame timestamp (UNIX ns).
    pub last_seen_ns: u64,
    /// Media/telemetry fan-out to subscribers.
    pub sfu: SfuFanout,
    /// Raw-frame session recorder.
    pub recorder: Box<dyn Recorder>,
}

impl UnitState {
    /// A flattened, serializable snapshot for the dashboard / MCP.
    pub fn info(&self) -> UnitInfo {
        UnitInfo {
            id: self.id,
            addr: self.addr.to_string(),
            mode: self.mode.name().to_string(),
            assigned_operator: self.assigned_operator.clone(),
            last_seq: self.last_seq,
            frames_recorded: self.recorder.frames(),
            subscribers: self.sfu.subscriber_count(),
            last_seen_ns: self.last_seen_ns,
        }
    }
}

/// Serializable unit snapshot.
#[derive(Debug, Clone, PartialEq)]
pub struct UnitInfo {
    /// Unit id.
    pub id: u64,
    /// Address string.
    pub addr: String,
    /// Mode name (`AUTO`/`ASSIST`/`TELEOP`/`ESTOP`).
    pub mode: String,
    /// Assigned operator (if any).
    pub assigned_operator: Option<String>,
    /// Last ingested sequence.
    pub last_seq: u64,
    /// Frames recorded this session.
    pub frames_recorded: u64,
    /// Current subscriber count (SFU viewers).
    pub subscribers: usize,
    /// Last inbound activity (UNIX ns).
    pub last_seen_ns: u64,
}

impl UnitInfo {
    /// Renders the snapshot as a JSON object.
    pub fn to_json(&self) -> Json {
        Json::obj(&[
            ("id", Json::uint(self.id)),
            ("addr", Json::str(&self.addr)),
            ("mode", Json::str(&self.mode)),
            (
                "assigned_operator",
                match &self.assigned_operator {
                    Some(o) => Json::str(o),
                    None => Json::Null,
                },
            ),
            ("last_seq", Json::uint(self.last_seq)),
            ("frames_recorded", Json::uint(self.frames_recorded)),
            ("subscribers", Json::uint(self.subscribers as u64)),
            ("last_seen_ns", Json::uint(self.last_seen_ns)),
        ])
    }
}

/// The fleet: many concurrent unit sessions behind one orchestrator.
pub struct Fleet {
    units: HashMap<u64, UnitState>,
    transport: Box<dyn UnitTransport>,
    table: TransitionTable,
    record_dir: Option<PathBuf>,
}

impl Fleet {
    /// Creates an empty fleet using `transport` for outbound commands.
    pub fn new(transport: Box<dyn UnitTransport>) -> Self {
        Self {
            units: HashMap::new(),
            transport,
            table: TransitionTable::spec_default(),
            record_dir: None,
        }
    }

    /// Enables on-disk session recording: newly provisioned units get a
    /// [`FileRecorder`] under `dir`.
    pub fn set_record_dir(&mut self, dir: PathBuf) {
        self.record_dir = Some(dir);
    }

    /// Explicitly registers a unit with a caller-supplied recorder.
    pub fn register_unit(
        &mut self,
        id: u64,
        addr: SocketAddr,
        recorder: Box<dyn Recorder>,
    ) -> Result<(), CloudError> {
        if self.units.contains_key(&id) {
            return Err(CloudError::AlreadyExists(id));
        }
        let now = crate::now_unix_ns();
        self.units.insert(
            id,
            UnitState {
                id,
                addr,
                mode: Mode::Assist,
                assigned_operator: None,
                last_seq: 0,
                cmd_seq: 0,
                created_ns: now,
                last_seen_ns: now,
                sfu: SfuFanout::new(),
                recorder,
            },
        );
        Ok(())
    }

    /// Auto-provisions a unit on first contact (used when a unit simply starts
    /// streaming). Updates the address and last-seen on subsequent contacts.
    fn provision(&mut self, id: u64, addr: SocketAddr) {
        if let Some(u) = self.units.get_mut(&id) {
            u.addr = addr;
            u.last_seen_ns = crate::now_unix_ns();
            return;
        }
        let recorder: Box<dyn Recorder> = match &self.record_dir {
            Some(dir) => match FileRecorder::create(dir, id) {
                Ok(r) => Box::new(r),
                Err(_) => Box::new(NullRecorder::new()),
            },
            None => Box::new(NullRecorder::new()),
        };
        let now = crate::now_unix_ns();
        self.units.insert(
            id,
            UnitState {
                id,
                addr,
                mode: Mode::Assist,
                assigned_operator: None,
                last_seq: 0,
                cmd_seq: 0,
                created_ns: now,
                last_seen_ns: now,
                sfu: SfuFanout::new(),
                recorder,
            },
        );
    }

    /// All unit snapshots.
    pub fn list_units(&self) -> Vec<UnitInfo> {
        self.units.values().map(|u| u.info()).collect()
    }

    /// Immutable unit lookup.
    pub fn get(&self, id: u64) -> Option<&UnitState> {
        self.units.get(&id)
    }

    /// Assigns an operator to a unit.
    pub fn assign(&mut self, id: u64, operator: String) -> Result<(), CloudError> {
        let u = self.units.get_mut(&id).ok_or(CloudError::NotFound(id))?;
        u.assigned_operator = Some(operator);
        Ok(())
    }

    /// Computes a legal staged transition path `from → to`, or `None` if the
    /// transition table forbids it even through `Assist`.
    fn path_via(&self, from: Mode, to: Mode) -> Option<Vec<Mode>> {
        if from == to {
            return Some(Vec::new());
        }
        if self.table.allows(from, to) {
            return Some(vec![to]);
        }
        // Staged handover through Assist (the spec's default policy).
        if from != Mode::Assist
            && to != Mode::Assist
            && self.table.allows(from, Mode::Assist)
            && self.table.allows(Mode::Assist, to)
        {
            return Some(vec![Mode::Assist, to]);
        }
        None
    }

    /// Commands a unit into `target` mode, enforcing the transition table.
    /// Staged paths (e.g. `Auto ↔ FullTeleop` via `Assist`) emit one command
    /// per segment.
    pub fn set_mode(&mut self, id: u64, target: Mode) -> Result<(), CloudError> {
        let from = self.units.get(&id).ok_or(CloudError::NotFound(id))?.mode;
        let path = self
            .path_via(from, target)
            .ok_or(CloudError::ModeDisallowed { from, to: target })?;
        let addr = self.units.get(&id).unwrap().addr;
        let u = self.units.get_mut(&id).unwrap();
        for &next in &path {
            let cmd = make_command(next, u.cmd_seq);
            self.transport
                .send_command(addr, &cmd)
                .map_err(|e| transport_err(addr, e))?;
            u.mode = next;
            u.cmd_seq = u.cmd_seq.wrapping_add(1);
        }
        Ok(())
    }

    /// Commands a unit into [`Mode::Auto`].
    pub fn engage_autonomy(&mut self, id: u64) -> Result<(), CloudError> {
        self.set_mode(id, Mode::Auto)
    }

    /// Commands a unit into [`Mode::FullTeleop`].
    pub fn take_manual_control(&mut self, id: u64) -> Result<(), CloudError> {
        self.set_mode(id, Mode::FullTeleop)
    }

    /// Ingests one frame for a unit: records it and fans it out to subscribers.
    /// Unknown units are auto-provisioned first.
    pub fn ingest(
        &mut self,
        id: u64,
        addr: SocketAddr,
        channel: u8,
        seq: u64,
        payload: &[u8],
    ) -> Result<(), CloudError> {
        self.provision(id, addr);
        let u = self.units.get_mut(&id).unwrap();
        u.last_seq = seq;
        u.last_seen_ns = crate::now_unix_ns();
        u.recorder
            .record(channel, seq, payload)
            .map_err(|e| CloudError::Recorder(e.to_string()))?;
        let frame = MediaFrame::new(channel, seq, payload);
        u.sfu.publish(frame);
        Ok(())
    }

    /// Attaches a media subscriber to a unit, returning its id and ring.
    pub fn attach_subscriber(
        &mut self,
        id: u64,
        capacity: usize,
    ) -> Result<(SubscriberId, Arc<SpscRing<MediaFrame>>), CloudError> {
        let u = self.units.get_mut(&id).ok_or(CloudError::NotFound(id))?;
        Ok(u.sfu.subscribe(capacity))
    }

    /// Live subscriber count for a unit.
    pub fn subscriber_count(&self, id: u64) -> Result<usize, CloudError> {
        self.units
            .get(&id)
            .map(|u| u.sfu.subscriber_count())
            .ok_or(CloudError::NotFound(id))
    }
}

/// Builds a control command for `mode` with the given sequence.
fn make_command(mode: Mode, seq: u64) -> ControlCommand {
    let mut c = ControlCommand::zeroed(mode);
    c.seq = seq;
    c.timestamp_ns = crate::now_unix_ns();
    c
}

/// Parses a mode name as used by the dashboard/MCP (`auto`, `assist`,
/// `teleop`/`fullteleop`, `estop`).
pub fn parse_mode(name: &str) -> Option<Mode> {
    match name.to_ascii_lowercase().as_str() {
        "auto" => Some(Mode::Auto),
        "assist" => Some(Mode::Assist),
        "teleop" | "fullteleop" | "full_teleop" => Some(Mode::FullTeleop),
        "estop" | "emergency" | "emergencystop" => Some(Mode::EmergencyStop),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recorder::VecRecorder;
    use std::net::{IpAddr, Ipv4Addr};

    fn addr() -> SocketAddr {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 5000)
    }

    #[test]
    fn register_and_list() {
        let mut f = Fleet::new(Box::new(NullTransport));
        f.register_unit(7, addr(), Box::new(VecRecorder::new()))
            .unwrap();
        assert_eq!(f.list_units().len(), 1);
        assert_eq!(f.get(7).unwrap().id, 7);
        assert!(
            f.register_unit(7, addr(), Box::new(VecRecorder::new()))
                .is_err()
        );
    }

    #[test]
    fn assign_operator() {
        let mut f = Fleet::new(Box::new(NullTransport));
        f.register_unit(1, addr(), Box::new(VecRecorder::new()))
            .unwrap();
        f.assign(1, "op-42".into()).unwrap();
        assert_eq!(
            f.get(1).unwrap().assigned_operator.as_deref(),
            Some("op-42")
        );
    }

    #[test]
    fn set_mode_direct_and_staged() {
        let mut f = Fleet::new(Box::new(CapturingTransport::new()));
        f.register_unit(1, addr(), Box::new(VecRecorder::new()))
            .unwrap();
        // Assist -> Auto is direct.
        f.set_mode(1, Mode::Auto).unwrap();
        assert_eq!(f.get(1).unwrap().mode, Mode::Auto);
        // Auto -> FullTeleop must stage through Assist (two commands).
        f.set_mode(1, Mode::FullTeleop).unwrap();
        assert_eq!(f.get(1).unwrap().mode, Mode::FullTeleop);
        let any: &dyn std::any::Any = &*f.transport;
        let sent = &any.downcast_ref::<CapturingTransport>().unwrap().sent;
        assert_eq!(sent.len(), 3, "Assist->Auto then Auto->Assist->FullTeleop");
        assert_eq!(sent[1].1.mode(), Some(Mode::Assist));
        assert_eq!(sent[2].1.mode(), Some(Mode::FullTeleop));
    }

    #[test]
    fn staged_transition_routes_through_assist() {
        // The spec default table forbids a *direct* Auto <-> FullTeleop jump
        // but permits the staged path through Assist, so set_mode emits one
        // command per segment (Auto -> Assist -> FullTeleop).
        let mut f = Fleet::new(Box::new(CapturingTransport::new()));
        f.register_unit(1, addr(), Box::new(VecRecorder::new()))
            .unwrap();
        f.set_mode(1, Mode::Auto).unwrap();
        f.set_mode(1, Mode::FullTeleop).unwrap();
        assert_eq!(f.get(1).unwrap().mode, Mode::FullTeleop);
        let any: &dyn std::any::Any = &*f.transport;
        let sent = &any.downcast_ref::<CapturingTransport>().unwrap().sent;
        // Assist->Auto (1) then Auto->Assist->FullTeleop (2) = 3 commands.
        assert_eq!(
            sent.len(),
            3,
            "staged transitions emit one command per segment"
        );
        assert_eq!(sent[1].1.mode(), Some(Mode::Assist));
        assert_eq!(sent[2].1.mode(), Some(Mode::FullTeleop));
    }

    #[test]
    fn emergency_stop_exits_only_through_assist() {
        let mut f = Fleet::new(Box::new(CapturingTransport::new()));
        f.register_unit(1, addr(), Box::new(VecRecorder::new()))
            .unwrap();
        f.set_mode(1, Mode::EmergencyStop).unwrap();
        assert_eq!(f.get(1).unwrap().mode, Mode::EmergencyStop);
        // From ESTOP the only legal edge is into Assist; reaching FullTeleop
        // therefore stages ESTOP -> Assist -> FullTeleop.
        f.set_mode(1, Mode::FullTeleop).unwrap();
        assert_eq!(f.get(1).unwrap().mode, Mode::FullTeleop);
    }

    #[test]
    fn ingest_records_and_routes() {
        let mut f = Fleet::new(Box::new(NullTransport));
        f.register_unit(1, addr(), Box::new(VecRecorder::new()))
            .unwrap();
        let (_, ring) = f.attach_subscriber(1, 16).unwrap();
        f.ingest(1, addr(), 2, 9, b"telemetry-bytes").unwrap();
        assert_eq!(f.get(1).unwrap().last_seq, 9);
        assert_eq!(f.get(1).unwrap().recorder.frames(), 1);
        let got = ring.pop().unwrap();
        assert_eq!(got.seq, 9);
        assert_eq!(got.bytes(), b"telemetry-bytes");
    }

    #[test]
    fn ingest_auto_provisions_unknown_unit() {
        let mut f = Fleet::new(Box::new(NullTransport));
        f.ingest(99, addr(), 3, 1, b"x").unwrap();
        assert!(f.get(99).is_some());
        assert_eq!(f.get(99).unwrap().recorder.frames(), 1);
    }
}
