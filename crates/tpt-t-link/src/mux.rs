//! The custom UDP multiplexer (spec §5.2): control, telemetry, media,
//! WebRTC ICE, and mesh traffic over **one** port.
//!
//! Datagram layout (little detail is left implicit — every field checked on
//! receive):
//!
//! ```text
//! ┌────────────────────────────────────────────────────────────┐
//! │ WireFrame 8B │ chan │ flags │ rsvd u16 │ route u32 (rsvd) │
//! └────────────────────────────────────────────────────────────┘
//! ├─────────────── HEADER_LEN = 16 ────────────────────────────┤
//! ┌──────────────────────────────────────────────────────────┐
//! │ payload (`payload_len` bytes, rkyv-encoded for our types)│
//! └──────────────────────────────────────────────────────────┘
//! ┌──────────────┐
//! │ CRC32 trailer│ 4B over header+payload
//! └──────────────┘
//! ```
//!
//! Serialization writes rkyv bytes into a reused aligned scratch buffer
//! ([`tpt_t_core::ser::AlignedBuf`]) and the framed datagram is assembled in
//! the caller's submission-owned packet buffer: zero heap allocation in
//! steady state, exactly one bounded memcpy from scratch to datagram.
//!
//! Default port is UDP 443 per spec; ICE passthrough frames carry opaque
//! WebRTC/STUN/DTLS bytes verbatim on [`Channel::Ice`].

use core::fmt;
use std::io;
use std::net::{SocketAddr, UdpSocket};

use tpt_t_core::ser::{
    AlignedBuf, ControlCommand, FRAME_MAGIC, PROTOCOL_VERSION, TelemetryPacket, WireFrame,
    access_root, serialize_into,
};
use tpt_t_ring::cast::{bytes_of, ref_from_bytes};

use crate::backpressure::Backpressure;
use crate::crc::{command_crc, crc32};
use crate::mesh::MeshBeacon;
use crate::reliable::AckFrame;

/// Service port from the spec ("UDP 443").
pub const DEFAULT_PORT: u16 = 443;

/// Largest datagram this link will send or accept (classic MTU).
pub const MAX_DATAGRAM: usize = 1500;

/// Framing overhead before payload.
///
/// 16 bytes so the payload begins 8-byte-aligned inside any reasonably
/// aligned receive buffer — required for zero-copy `rkyv::access` views of
/// structs containing `u64`/`f64` fields. Bytes 12..16 are a reserved
/// routing word (unit/session addressing lands with spec §5.6).
pub const HEADER_LEN: usize = 16;

/// CRC32 trailer length.
pub const TRAILER_LEN: usize = 4;

/// Largest payload that fits one datagram.
pub const MAX_PAYLOAD: usize = MAX_DATAGRAM - HEADER_LEN - TRAILER_LEN;

/// Multiplexed channel discriminator (byte 8 of every datagram).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Channel {
    /// Safety-approved operator commands (rkyv `ControlCommand`).
    Control = 1,
    /// Sensor/telemetry packets (rkyv `TelemetryPacket`).
    Telemetry = 2,
    /// Encoded media slices (Phase 8 wires this to the encoder).
    Media = 3,
    /// Opaque ICE/STUN/DTLS passthrough toward a WebRTC stack.
    Ice = 4,
    /// Swarm neighbor-discovery beacons ([`MeshBeacon`]).
    Mesh = 5,
}

impl Channel {
    /// Discriminant.
    #[inline]
    pub fn as_u8(self) -> u8 {
        self as u8
    }

    /// Inverse of [`as_u8`](Self::as_u8); `None` for unknown values.
    #[inline]
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            1 => Some(Channel::Control),
            2 => Some(Channel::Telemetry),
            3 => Some(Channel::Media),
            4 => Some(Channel::Ice),
            5 => Some(Channel::Mesh),
            _ => None,
        }
    }
}

/// Frame flag bits (byte 9).
pub mod flags {
    /// Payload is a [`reliable::AckFrame`](crate::reliable::AckFrame)
    /// answering a reliable control frame.
    pub const ACK: u8 = 0b0000_0001;
    /// Sent through the reliable control channel (seq = `cmd.seq`).
    pub const RELIABLE: u8 = 0b0000_0010;
}

/// Atomic link counters (network thread writes; readers are lock-free).
#[derive(Debug, Default)]
pub struct LinkStats {
    /// Datagrams sent successfully.
    pub tx_frames: core::sync::atomic::AtomicU64,
    /// Payload bytes sent (excludes framing).
    pub tx_payload_bytes: core::sync::atomic::AtomicU64,
    /// Send syscalls that failed (incl. `EWOULDBLOCK`).
    pub tx_errors: core::sync::atomic::AtomicU64,
    /// Datagrams received and demultiplexed.
    pub rx_frames: core::sync::atomic::AtomicU64,
    /// Bad magic / version / length rejects.
    pub rx_malformed: core::sync::atomic::AtomicU64,
    /// CRC trailer mismatches.
    pub rx_crc_errors: core::sync::atomic::AtomicU64,
    /// Unknown channel or failed payload validation.
    pub rx_rejected: core::sync::atomic::AtomicU64,
}

impl LinkStats {
    /// Increments one counter (relaxed order — counters are advisory).
    pub fn bump(cell: &core::sync::atomic::AtomicU64) {
        let _ = cell.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    }
}

/// One fully-demultiplexed inbound frame. Payload references borrow the
/// caller's receive scratch buffer — zero-copy until the consumer decides
/// otherwise.
pub enum Inbound<'a> {
    /// Validated control command (archived view into the scratch buffer).
    Control {
        /// Archived command (fields read directly; POD).
        cmd: &'a <ControlCommand as rkyv::Archive>::Archived,
        /// Sender address.
        from: SocketAddr,
        /// True when [`flags::RELIABLE`] was set (ack expected).
        reliable: bool,
    },
    /// Validated telemetry packet.
    Telemetry {
        /// Archived packet.
        pkt: &'a <TelemetryPacket as rkyv::Archive>::Archived,
        /// Sender address.
        from: SocketAddr,
    },
    /// Opaque ICE/STUN/DTLS bytes.
    Ice {
        /// Raw passthrough payload.
        payload: &'a [u8],
        /// Sender address.
        from: SocketAddr,
    },
    /// Decoded mesh beacon.
    Mesh {
        /// The beacon (POD copy out of the validated cast).
        beacon: MeshBeacon,
        /// Sender address (the beacon's listen port is authoritative for
        /// replies, not this source port).
        from: SocketAddr,
    },
    /// Reliable-channel acknowledgement.
    Ack {
        /// The ack frame.
        ack: AckFrame,
        /// Sender address.
        from: SocketAddr,
    },
}

impl fmt::Debug for Inbound<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Archived payloads lack Debug impls by design (raw POD views); the
        // variant plus peer address is what logs need anyway.
        match self {
            Inbound::Control { from, reliable, .. } => f
                .debug_struct("Control")
                .field("from", from)
                .field("reliable", reliable)
                .finish(),
            Inbound::Telemetry { from, .. } => {
                f.debug_struct("Telemetry").field("from", from).finish()
            }
            Inbound::Ice { payload, from } => f
                .debug_struct("Ice")
                .field("len", &payload.len())
                .field("from", from)
                .finish(),
            Inbound::Mesh { beacon, from } => f
                .debug_struct("Mesh")
                .field("unit_id", &beacon.unit_id)
                .field("from", from)
                .finish(),
            Inbound::Ack { ack, from } => f
                .debug_struct("Ack")
                .field("base_seq", &ack.base_seq)
                .field("from", from)
                .finish(),
        }
    }
}

/// UDP multiplexer bound to one port. Not `Sync`: owns the socket and is
/// driven by exactly one network thread (the Phase 2 event-loop thread).
pub struct UdpMux {
    socket: UdpSocket,
    scratch: AlignedBuf,
    /// Counters and congestion signal shared with other threads.
    pub stats: LinkStats,
    /// Bandwidth admission/congestion state (also fed by reliable RTT).
    pub backpressure: Backpressure,
}

impl fmt::Debug for UdpMux {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UdpMux")
            .field("local_addr", &self.socket.local_addr().ok())
            .finish()
    }
}

/// 16-byte-aligned receive buffer.
///
/// rkyv's zero-copy [`access_root`](tpt_t_core::ser::access_root) validation
/// requires the archived bytes to sit at the type's alignment, and a payload
/// lands at offset 12 inside a plain `[u8; MAX_DATAGRAM]` — unaligned by
/// definition. Allocating receives through this wrapper makes every archived
/// view soundly readable regardless of what the payload offset does.
#[repr(C, align(16))]
pub struct RxBuffer {
    buf: [u8; MAX_DATAGRAM],
}

impl RxBuffer {
    /// Zero-initialized buffer.
    pub fn new() -> Self {
        Self {
            buf: [0u8; MAX_DATAGRAM],
        }
    }
}

impl Default for RxBuffer {
    fn default() -> Self {
        Self::new()
    }
}

impl core::fmt::Debug for RxBuffer {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("RxBuffer(..)")
    }
}

impl core::ops::Deref for RxBuffer {
    type Target = [u8];
    #[inline]
    fn deref(&self) -> &[u8] {
        &self.buf
    }
}

impl core::ops::DerefMut for RxBuffer {
    #[inline]
    fn deref_mut(&mut self) -> &mut [u8] {
        &mut self.buf
    }
}

impl UdpMux {
    /// Binds to `0.0.0.0:port` (use [`DEFAULT_PORT`] for the spec port,
    /// [`UdpMux::bind_ephemeral`] for peer-only roles, or
    /// [`UdpMux::bind_loopback`] in tests). The socket is non-blocking so it
    /// composes with the Phase 2 event loops.
    pub fn bind(port: u16) -> io::Result<Self> {
        Self::bind_on("0.0.0.0", port)
    }

    /// Binds to an OS-assigned port on all interfaces.
    pub fn bind_ephemeral() -> io::Result<Self> {
        Self::bind(0)
    }

    /// Binds loopback-only on an OS-assigned port (unit tests).
    pub fn bind_loopback() -> io::Result<Self> {
        Self::bind_on("127.0.0.1", 0)
    }

    fn bind_on(host: &str, port: u16) -> io::Result<Self> {
        let socket = UdpSocket::bind((host, port))?;
        socket.set_nonblocking(true)?;
        Ok(Self {
            socket,
            scratch: AlignedBuf::new(),
            stats: LinkStats::default(),
            backpressure: Backpressure::default(),
        })
    }

    /// Local bound address.
    #[inline]
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.socket.local_addr()
    }

    /// Raw fd (Unix) for event-loop registration.
    #[cfg(unix)]
    #[inline]
    pub fn as_raw_fd(&self) -> i32 {
        use std::os::fd::AsRawFd;
        self.socket.as_raw_fd()
    }

    /// Raw HANDLE value (Windows) for IOCP registration.
    #[cfg(windows)]
    #[inline]
    pub fn as_raw_handle_value(&self) -> usize {
        use std::os::windows::io::AsRawSocket;
        self.socket.as_raw_socket() as usize
    }

    // -- framing ----------------------------------------------------------

    /// Writes the 16-byte header into `out[0..HEADER_LEN]`. Bytes 12..16
    /// are the reserved routing word (zero until §5.6 session addressing).
    fn write_header(out: &mut [u8], channel: Channel, frame_flags: u8, payload_len: u16) {
        let hdr = WireFrame {
            magic: FRAME_MAGIC,
            version: PROTOCOL_VERSION,
            payload_len,
        };
        out[..8].copy_from_slice(bytes_of(&hdr));
        out[8] = channel.as_u8();
        out[9] = frame_flags;
        out[10..HEADER_LEN].fill(0);
    }

    /// Appends the CRC32 trailer over `out[..end]` and returns total length.
    fn finish_frame(out: &mut [u8], end: usize) -> usize {
        let sum = crc32(&out[..end]);
        out[end..end + TRAILER_LEN].copy_from_slice(&sum.to_le_bytes());
        end + TRAILER_LEN
    }

    /// Assembles a control datagram: stamps the command's `crc` field,
    /// rkyv-serializes into the reused aligned scratch, frames, tags.
    /// Returns bytes written into `out`.
    pub fn write_control_frame(
        &mut self,
        cmd: &ControlCommand,
        frame_flags: u8,
        out: &mut [u8; MAX_DATAGRAM],
    ) -> io::Result<usize> {
        debug_assert!(frame_flags & flags::ACK == 0, "acks use write_ack_frame");
        let mut stamped = *cmd;
        stamped.crc = command_crc(cmd);
        let n = serialize_into(&stamped, &mut self.scratch).map_err(io::Error::other)?;
        if n > MAX_PAYLOAD {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "control payload exceeds MTU",
            ));
        }
        Self::write_header(out, Channel::Control, frame_flags, n as u16);
        out[HEADER_LEN..HEADER_LEN + n].copy_from_slice(&self.scratch[..n]);
        Ok(Self::finish_frame(out, HEADER_LEN + n))
    }

    /// Assembles a telemetry datagram.
    pub fn write_telemetry_frame(
        &mut self,
        pkt: &TelemetryPacket,
        out: &mut [u8; MAX_DATAGRAM],
    ) -> io::Result<usize> {
        let n = serialize_into(pkt, &mut self.scratch).map_err(io::Error::other)?;
        if n > MAX_PAYLOAD {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "telemetry payload exceeds MTU",
            ));
        }
        Self::write_header(out, Channel::Telemetry, 0, n as u16);
        out[HEADER_LEN..HEADER_LEN + n].copy_from_slice(&self.scratch[..n]);
        Ok(Self::finish_frame(out, HEADER_LEN + n))
    }
}

impl UdpMux {
    /// Assembles a mesh beacon datagram.
    pub fn write_mesh_frame(
        &mut self,
        beacon: &MeshBeacon,
        out: &mut [u8; MAX_DATAGRAM],
    ) -> io::Result<usize> {
        let n = beacon.encode_into(&mut out[HEADER_LEN..]);
        Self::write_header(out, Channel::Mesh, 0, n as u16);
        Ok(Self::finish_frame(out, HEADER_LEN + n))
    }

    /// Assembles a reliable-channel ack datagram.
    pub fn write_ack_frame(
        &mut self,
        ack: &AckFrame,
        out: &mut [u8; MAX_DATAGRAM],
    ) -> io::Result<usize> {
        let n = ack.encode_into(&mut out[HEADER_LEN..]);
        Self::write_header(out, Channel::Control, flags::ACK, n as u16);
        Ok(Self::finish_frame(out, HEADER_LEN + n))
    }

    /// Assembles an ICE passthrough datagram (`bytes` forwarded verbatim).
    /// WebRTC stacks keep their own fragments below MTU; oversized input is
    /// rejected rather than silently truncated.
    pub fn write_ice_frame(
        &mut self,
        bytes: &[u8],
        out: &mut [u8; MAX_DATAGRAM],
    ) -> io::Result<usize> {
        if bytes.len() > MAX_PAYLOAD {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "ICE payload exceeds MTU",
            ));
        }
        out[HEADER_LEN..HEADER_LEN + bytes.len()].copy_from_slice(bytes);
        Self::write_header(out, Channel::Ice, 0, bytes.len() as u16);
        Ok(Self::finish_frame(out, HEADER_LEN + bytes.len()))
    }

    // -- transmit ---------------------------------------------------------

    /// Sends an already-framed buffer; feeds stats/backpressure.
    pub fn send_framed(&mut self, dst: SocketAddr, buf: &[u8], now_ns: u64) -> io::Result<usize> {
        match self.socket.send_to(buf, dst) {
            Ok(n) => {
                LinkStats::bump(&self.stats.tx_frames);
                let _ = self.stats.tx_payload_bytes.fetch_add(
                    n.saturating_sub(HEADER_LEN + TRAILER_LEN) as u64,
                    core::sync::atomic::Ordering::Relaxed,
                );
                Ok(n)
            }
            Err(e)
                if e.kind() == io::ErrorKind::WouldBlock || e.kind() == io::ErrorKind::TimedOut =>
            {
                LinkStats::bump(&self.stats.tx_errors);
                self.backpressure.note_send_blocked(now_ns);
                Err(e)
            }
            Err(e) => {
                LinkStats::bump(&self.stats.tx_errors);
                Err(e)
            }
        }
    }
}

/// Parse failure classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameError {
    /// Bad size / magic / version / declared length.
    Malformed,
    /// Trailer or command CRC mismatch.
    Crc,
    /// Unknown channel or payload failed type validation.
    Payload,
}

impl core::fmt::Display for FrameError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            FrameError::Malformed => f.write_str("malformed frame"),
            FrameError::Crc => f.write_str("crc mismatch"),
            FrameError::Payload => f.write_str("payload rejected"),
        }
    }
}

impl std::error::Error for FrameError {}

impl UdpMux {
    /// Receives exactly one datagram into `scratch` and demultiplexes it.
    ///
    /// * `Ok(None)` — socket would block.
    /// * `Ok(Some(Ok(inbound)))` — valid frame; references borrow `scratch`.
    /// * `Ok(Some(Err(e)))` — corrupt/misaddressed frame, classified in `e`
    ///   (callers decide whether to keep draining).
    ///
    /// The drain-until-valid policy deliberately lives with the caller: the
    /// network service counts rejects into [`UdpMux::stats`] while looping,
    /// which keeps this method free of internal borrow-carried state.
    pub fn recv_frame<'a>(
        &mut self,
        scratch: &'a mut RxBuffer,
    ) -> io::Result<Option<Result<Inbound<'a>, FrameError>>> {
        let Some((n, from)) = self.fill(scratch)? else {
            return Ok(None);
        };
        let step = Self::demux(scratch, n, from);
        if step.is_ok() {
            LinkStats::bump(&self.stats.rx_frames);
        }
        Ok(Some(step))
    }

    /// One raw receive into `scratch`; `None` on would-block. Keeps the
    /// socket borrow out of the parse path so frame references can alias the
    /// scratch buffer across the demux match.
    // The retry loop only iterates on Windows (WSAECONNRESET noise); on
    // other platforms every branch returns, which clippy flags as a
    // never-loop — that is exactly the intent.
    #[allow(clippy::never_loop)]
    fn fill(&mut self, scratch: &mut RxBuffer) -> io::Result<Option<(usize, SocketAddr)>> {
        loop {
            match self.socket.recv_from(scratch) {
                Ok(v) => return Ok(Some(v)),
                Err(e)
                    if e.kind() == io::ErrorKind::WouldBlock
                        || e.kind() == io::ErrorKind::TimedOut =>
                {
                    return Ok(None);
                }
                Err(e) => {
                    // Windows WSAECONNRESET surfaces on ICMP-unreachable
                    // feedback for prior sends; treat as transient noise.
                    #[cfg(windows)]
                    {
                        if e.raw_os_error() == Some(10054) {
                            LinkStats::bump(&self.stats.tx_errors);
                            continue;
                        }
                    }
                    return Err(e);
                }
            }
        }
    }

    /// Pure frame parser: validates header, trailer CRC, and channel payload;
    /// returns borrowed views into `buf`. No I/O, no counters — callers own
    /// the accounting.
    fn demux<'a>(buf: &'a RxBuffer, n: usize, from: SocketAddr) -> Result<Inbound<'a>, FrameError> {
        if n < HEADER_LEN + TRAILER_LEN {
            return Err(FrameError::Malformed);
        }
        // Header checks before spending cycles on the CRC.
        let hdr: &WireFrame =
            ref_from_bytes::<WireFrame>(&buf[..8]).map_err(|_| FrameError::Malformed)?;
        if hdr.magic != FRAME_MAGIC || hdr.version != PROTOCOL_VERSION {
            return Err(FrameError::Malformed);
        }
        let payload_len = hdr.payload_len as usize;
        let end = HEADER_LEN + payload_len;
        let total = end + TRAILER_LEN;
        if total != n {
            return Err(FrameError::Malformed);
        }
        // Trailer CRC covers header + payload.
        let expect = u32::from_le_bytes(buf[end..total].try_into().expect("trailer is 4 bytes"));
        if crc32(&buf[..end]) != expect {
            return Err(FrameError::Crc);
        }
        let payload = &buf[HEADER_LEN..end];
        match Channel::from_u8(buf[8]) {
            Some(Channel::Control) => {
                if buf[9] & flags::ACK != 0 {
                    AckFrame::decode(payload)
                        .map(|ack| Inbound::Ack { ack, from })
                        .ok_or(FrameError::Payload)
                } else {
                    match access_root::<ControlCommand>(payload) {
                        Ok(arch) => {
                            let owned = Self::command_from_archived(arch);
                            if command_crc(&owned) == owned.crc {
                                Ok(Inbound::Control {
                                    cmd: arch,
                                    from,
                                    reliable: buf[9] & flags::RELIABLE != 0,
                                })
                            } else {
                                Err(FrameError::Crc)
                            }
                        }
                        Err(_) => Err(FrameError::Payload),
                    }
                }
            }
            Some(Channel::Telemetry) => access_root::<TelemetryPacket>(payload)
                .map(|pkt| Inbound::Telemetry { pkt, from })
                .map_err(|_| FrameError::Payload),
            Some(Channel::Ice) => Ok(Inbound::Ice { payload, from }),
            Some(Channel::Mesh) => MeshBeacon::decode(payload)
                .map(|beacon| Inbound::Mesh { beacon, from })
                .ok_or(FrameError::Payload),
            // Media demux lands with Phase 8; unknown channels are
            // protocol-version skew — drop loudly via counters.
            Some(Channel::Media) | None => Err(FrameError::Payload),
        }
    }

    /// Rebuilds an owned [`ControlCommand`] from its archived form (56-byte
    /// stack copy) so hop-integrity CRCs can be recomputed identically to
    /// the sender. Archived integers are little-endian wrappers (`u32_le`),
    /// hence the `Into` conversions.
    pub fn command_from_archived(
        arch: &<ControlCommand as rkyv::Archive>::Archived,
    ) -> ControlCommand {
        ControlCommand {
            magic: arch.magic.into(),
            version: arch.version.into(),
            reserved: arch.reserved.into(),
            seq: arch.seq.into(),
            timestamp_ns: arch.timestamp_ns.into(),
            mode: arch.mode,
            flags: arch.flags,
            axis_count: arch.axis_count,
            _pad0: arch._pad0,
            axes: arch.axes.map(|v| v.into()),
            crc: arch.crc.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reliable::AckFrame;
    use core::sync::atomic::Ordering;
    use std::net::Ipv4Addr;
    use tpt_t_core::mode::Mode;
    use tpt_t_core::ser::TelemetryKind;

    fn sample_cmd(seq: u64) -> ControlCommand {
        let mut c = ControlCommand::zeroed(Mode::FullTeleop);
        c.seq = seq;
        c.timestamp_ns = 1_000 + seq;
        c.axes[0] = 0.25;
        c.axes[3] = -0.5;
        c
    }

    /// Owned mirror of [`Inbound`] so test drains can loop freely.
    /// (Some peer-address fields are carried for symmetry even when a given
    /// test doesn't assert on them.)
    #[derive(Debug)]
    #[allow(dead_code)]
    enum Ev {
        Control(ControlCommand, SocketAddr, bool),
        Telemetry(TelemetryPacket, SocketAddr),
        Ice(Vec<u8>, SocketAddr),
        Mesh(MeshBeacon, SocketAddr),
        Ack(AckFrame, SocketAddr),
    }

    /// Drains until a valid frame arrives; converts it to owned data.
    fn next_ev(mux: &mut UdpMux, rx: &mut RxBuffer) -> Ev {
        loop {
            match mux.recv_frame(rx).unwrap() {
                Some(Ok(inb)) => {
                    return match inb {
                        Inbound::Control {
                            cmd,
                            from,
                            reliable,
                        } => Ev::Control(UdpMux::command_from_archived(cmd), from, reliable),
                        Inbound::Telemetry { pkt, from } => Ev::Telemetry(
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
                        Inbound::Ice { payload, from } => Ev::Ice(payload.to_vec(), from),
                        Inbound::Mesh { beacon, from } => Ev::Mesh(beacon, from),
                        Inbound::Ack { ack, from } => Ev::Ack(ack, from),
                    };
                }
                Some(Err(e)) => panic!("unexpected reject: {e}"),
                None => std::thread::sleep(core::time::Duration::from_millis(5)),
            }
        }
    }

    #[test]
    fn demux_direct_parse_is_alignment_safe() {
        use tpt_t_core::ser::{PROTOCOL_VERSION as PV, access_root};
        let mut a = UdpMux::bind_loopback().unwrap();
        let mut buf = [0u8; MAX_DATAGRAM];
        let n = a.write_control_frame(&sample_cmd(7), 0, &mut buf).unwrap();
        let hdr: &tpt_t_core::ser::WireFrame = ref_from_bytes(&buf[..8]).unwrap();
        assert_eq!(hdr.magic, tpt_t_core::ser::FRAME_MAGIC);
        assert_eq!(hdr.version, PV);
        let plen = hdr.payload_len as usize;

        // Properly aligned receive path.
        let mut rxbuf = RxBuffer::new();
        rxbuf[..n].copy_from_slice(&buf[..n]);
        match access_root::<ControlCommand>(&rxbuf[HEADER_LEN..HEADER_LEN + plen]) {
            Ok(_) => eprintln!("probe: access ok"),
            Err(e) => panic!("probe: access failed: {e}"),
        }
        let step = UdpMux::demux(&rxbuf, n, "127.0.0.1:1".parse().unwrap());
        assert!(step.is_ok(), "demux failed: {:?}", step.err());
    }

    #[test]
    fn control_frame_roundtrips_over_real_udp() {
        let mut a = UdpMux::bind_loopback().unwrap();
        let mut b = UdpMux::bind_loopback().unwrap();
        let dst = b.local_addr().unwrap();

        let mut buf = [0u8; MAX_DATAGRAM];
        let n = a
            .write_control_frame(&sample_cmd(7), flags::RELIABLE, &mut buf)
            .unwrap();
        assert!(n > HEADER_LEN + TRAILER_LEN);
        a.send_framed(dst, &buf[..n], 0).unwrap();

        let mut rx = RxBuffer::new();
        match next_ev(&mut b, &mut rx) {
            Ev::Control(cmd, from, reliable) => {
                assert_eq!(cmd.seq, 7);
                assert_eq!(cmd.mode(), Some(Mode::FullTeleop));
                assert_eq!(cmd.axes[0], 0.25);
                assert_eq!(cmd.axes[3], -0.5);
                assert!(reliable);
                assert_eq!(from.ip(), std::net::IpAddr::V4(Ipv4Addr::LOCALHOST));
                // Hop-integrity field survives the wire.
                assert_eq!(cmd.crc, command_crc(&sample_cmd(7)));
            }
            other => panic!("wrong channel demuxed: {other:?}"),
        }
        assert_eq!(a.stats.tx_frames.load(Ordering::Relaxed), 1);
        assert_eq!(b.stats.rx_frames.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn corrupted_frames_are_counted_and_skipped() {
        let mut a = UdpMux::bind_loopback().unwrap();
        let mut b = UdpMux::bind_loopback().unwrap();
        let dst = b.local_addr().unwrap();

        let mut buf = [0u8; MAX_DATAGRAM];
        let n = a.write_control_frame(&sample_cmd(1), 0, &mut buf).unwrap();

        // 1) CRC corruption.
        let mut bad_crc = buf;
        bad_crc[n - 1] ^= 0xFF;
        a.send_framed(dst, &bad_crc[..n], 0).unwrap();
        // 2) Magic corruption.
        let mut bad_magic = buf;
        bad_magic[2] ^= 0xFF;
        a.send_framed(dst, &bad_magic[..n], 0).unwrap();
        // 3) Length field lies about payload size.
        let mut bad_len = buf;
        bad_len[6] = bad_len[6].wrapping_add(1); // payload_len += 256
        a.send_framed(dst, &bad_len[..n], 0).unwrap();
        // 4) Good frame last — must still arrive through the garbage.
        a.send_framed(dst, &buf[..n], 0).unwrap();

        let mut rx = RxBuffer::new();
        // First three receives surface classified errors...
        let mut got = Vec::new();
        for _ in 0..50 {
            match b.recv_frame(&mut rx).unwrap() {
                Some(Ok(_)) => break,
                Some(Err(e)) => {
                    got.push(e);
                    if got.len() == 3 {
                        break;
                    }
                }
                None => std::thread::sleep(core::time::Duration::from_millis(5)),
            }
        }
        // ...then the good frame must still arrive through the garbage.
        match next_ev(&mut b, &mut rx) {
            Ev::Control(cmd, _, _) => assert_eq!(cmd.seq, 1),
            other => panic!("expected control frame, got {other:?}"),
        }
        assert_eq!(
            got,
            vec![
                FrameError::Crc,
                FrameError::Malformed,
                FrameError::Malformed
            ],
            "crc reject counted, then magic and length rejects"
        );
    }

    #[test]
    fn channels_demux_independently() {
        let mut a = UdpMux::bind_loopback().unwrap();
        let mut b = UdpMux::bind_loopback().unwrap();
        let dst = b.local_addr().unwrap();
        let mut buf = [0u8; MAX_DATAGRAM];

        // Telemetry
        let pkt = TelemetryPacket {
            values: [1.0; 8],
            ..TelemetryPacket::zeroed(TelemetryKind::Battery, 9, 10)
        };
        let n = a.write_telemetry_frame(&pkt, &mut buf).unwrap();
        a.send_framed(dst, &buf[..n], 0).unwrap();

        // ICE passthrough
        let n = a
            .write_ice_frame(&[0x16, 0xFE, 0xFD, 0xAA], &mut buf)
            .unwrap();
        a.send_framed(dst, &buf[..n], 0).unwrap();

        // Mesh
        let beacon = MeshBeacon::new(77, 3, 42, b.local_addr().unwrap().port());
        let n = a.write_mesh_frame(&beacon, &mut buf).unwrap();
        a.send_framed(dst, &buf[..n], 0).unwrap();

        // Ack
        let ack = AckFrame {
            base_seq: 5,
            bitmap: 0b11,
            rtt_sample_ns: 900,
        };
        let n = a.write_ack_frame(&ack, &mut buf).unwrap();
        a.send_framed(dst, &buf[..n], 0).unwrap();

        let mut rx = RxBuffer::new();
        let mut kinds = Vec::new();
        for _ in 0..4 {
            match next_ev(&mut b, &mut rx) {
                Ev::Telemetry(pkt, _) => {
                    assert_eq!(pkt.values[7], 1.0);
                    kinds.push(0);
                }
                Ev::Ice(payload, _) => {
                    assert_eq!(payload, vec![0x16, 0xFE, 0xFD, 0xAA]);
                    kinds.push(1);
                }
                Ev::Mesh(beacon, _) => {
                    assert_eq!(beacon.unit_id, 77);
                    assert_eq!(beacon.listen_port, dst.port());
                    kinds.push(2);
                }
                Ev::Ack(ack, _) => {
                    assert_eq!((ack.base_seq, ack.bitmap), (5, 0b11));
                    kinds.push(3);
                }
                Ev::Control(..) => panic!("stray control frame"),
            }
        }
        kinds.sort_unstable();
        assert_eq!(kinds, vec![0, 1, 2, 3]);
    }

    #[test]
    fn oversize_payloads_are_rejected_before_send() {
        let mut a = UdpMux::bind_loopback().unwrap();
        let mut buf = [0u8; MAX_DATAGRAM];
        let big = vec![0u8; MAX_PAYLOAD + 1];
        assert!(a.write_ice_frame(&big, &mut buf).is_err());
    }
}
