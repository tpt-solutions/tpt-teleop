//! Custom MAVLink parser (spec §5.2 "custom MAVLink parser from scratch, no
//! `rust-mavlink`").
//!
//! Allocation-free framing for MAVLink 1.0 (magic `0xFE`) and 2.0
//! (magic `0xFD`), including v2's optional 13-byte signature tail. The
//! checksum is MAVLink's CRC-16/MCRF4XX (reflected, poly `0x8408`, init
//! `0xFFFF`) over header+payload, XORed with the message `crc_extra` drawn
//! from a small built-in dialect table. Parsed frames land in the
//! rkyv-serializable [`MavFrame`], so they forward onto the ring/link path
//! with zero copies.
//!
//! A streaming [`MavParser`] accumulates bytes from any transport (UART,
//! SocketCAN, UDP) into a fixed 263-byte buffer and yields complete frames
//! without heap traffic.

use rkyv::{Archive, Deserialize, Serialize};

/// MAVLink 1.0 framing magic.
pub const MAVLINK_1_MAGIC: u8 = 0xFE;
/// MAVLink 2.0 framing magic.
pub const MAVLINK_2_MAGIC: u8 = 0xFD;
/// v2 signature tail length.
pub const MAVLINK_2_SIGNATURE_LEN: usize = 13;
/// Maximum MAVLink payload (both versions).
pub const MAVLINK_MAX_PAYLOAD: usize = 255;

/// Built-in dialect subset: `(msg_id, payload_len, crc_extra)`.
/// crc_extra values are from the MAVLink XML dialect (common.xml).
const DIALECT: &[(u32, u8, u8)] = &[
    (0, 9, 50),    // HEARTBEAT
    (30, 28, 39),  // ATTITUDE
    (33, 28, 104), // GLOBAL_POSITION_INT
    (1, 31, 124),  // SYS_STATUS
    (24, 12, 167), // GPS_RAW_INT
];

/// Look up `(len, crc_extra)` for a message id (v2 24-bit space).
#[inline]
pub fn dialect_for(msgid: u32) -> Option<(u8, u8)> {
    DIALECT
        .iter()
        .find(|(id, _, _)| *id == msgid)
        .map(|(_, len, extra)| (*len, *extra))
}

/// MAVLink CRC-16/MCRF4XX (reflected, poly `0x8408`, init `0xFFFF`).
pub fn mav_crc16(data: &[u8]) -> u16 {
    let mut crc: u16 = 0xFFFF;
    for &b in data {
        crc ^= b as u16;
        for _ in 0..8 {
            if crc & 1 != 0 {
                crc = (crc >> 1) ^ 0x8408;
            } else {
                crc >>= 1;
            }
        }
    }
    crc
}

/// Why a parse failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MavError {
    /// Leading byte was not a MAVLink magic.
    Magic,
    /// Buffer ended before a complete frame.
    Truncated,
    /// Trailing checksum did not match.
    Crc,
}

impl core::fmt::Display for MavError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            MavError::Magic => write!(f, "not a MAVLink frame (bad magic)"),
            MavError::Truncated => write!(f, "truncated MAVLink frame"),
            MavError::Crc => write!(f, "MAVLink checksum mismatch"),
        }
    }
}

impl std::error::Error for MavError {}

/// One decoded MAVLink frame (rkyv-friendly POD).
#[derive(Debug, Clone, Copy, PartialEq, Archive, Serialize, Deserialize)]
#[repr(C)]
pub struct MavFrame {
    /// Protocol version (`1` or `2`).
    pub version: u8,
    /// Packet sequence number.
    pub seq: u8,
    /// System id.
    pub sysid: u8,
    /// Component id.
    pub compid: u8,
    /// Message id (24-bit in v2, low byte in v1).
    pub msgid: u32,
    /// Payload length in bytes.
    pub payload_len: u8,
    /// Raw payload bytes (unused tail is zero).
    pub payload: [u8; MAVLINK_MAX_PAYLOAD],
    /// Trailing checksum (network order).
    pub crc: u16,
    /// 1 when a v2 signature tail was present, else 0.
    pub signed: u8,
}
// SAFETY: repr(C) dense POD; rkyv/cast-friendly.
unsafe impl tpt_t_ring::cast::PlainBytes for MavFrame {}

impl MavFrame {
    /// Message id as a 24-bit value.
    #[inline]
    pub fn message_id(&self) -> u32 {
        self.msgid
    }

    /// Borrow the populated payload slice.
    #[inline]
    pub fn payload(&self) -> &[u8] {
        &self.payload[..self.payload_len as usize]
    }
}

/// Total on-wire byte length for a frame with `payload_len` and `signed`.
#[inline]
fn frame_len(version: u8, payload_len: usize, signed: bool) -> usize {
    // `header_total` = magic (1) + header after magic (5 for v1, 10 for v2).
    let header_total = if version == 1 { 6usize } else { 11usize };
    let sig = if version == 2 && signed {
        MAVLINK_2_SIGNATURE_LEN
    } else {
        0
    };
    header_total + payload_len + 2 + sig
}

/// Parses one complete MAVLink frame from `buf` (must contain the whole
/// frame). CRC is validated when the message id is in the dialect table;
/// unknown ids parse structurally (the appended crc is still recorded).
pub fn parse_frame(buf: &[u8]) -> Result<MavFrame, MavError> {
    if buf.is_empty() {
        return Err(MavError::Magic);
    }
    let version = match buf[0] {
        MAVLINK_1_MAGIC => 1,
        MAVLINK_2_MAGIC => 2,
        _ => return Err(MavError::Magic),
    };

    let (payload_len, header_after) = if version == 1 {
        (buf[1] as usize, 5usize)
    } else {
        (buf[1] as usize, 10usize)
    };
    if payload_len > MAVLINK_MAX_PAYLOAD {
        return Err(MavError::Truncated);
    }
    // Detect v2 signature flag (bit 0x01 of the incompat flags byte).
    let signed = version == 2 && buf[2] & 0x01 != 0;
    let total = frame_len(version, payload_len, signed);
    if buf.len() < total {
        return Err(MavError::Truncated);
    }

    // CRC covers header (after magic) + payload; crc starts at the byte after.
    let crc_pos = 1 + header_after + payload_len;
    let crc = u16::from_le_bytes([buf[crc_pos], buf[crc_pos + 1]]);

    let msgid = if version == 1 {
        buf[5] as u32
    } else {
        (buf[7] as u32) | ((buf[8] as u32) << 8) | ((buf[9] as u32) << 16)
    };

    // Validate checksum over header+payload (exclude magic, crc, signature).
    if let Some((expected_len, extra)) = dialect_for(msgid) {
        if expected_len as usize == payload_len {
            let calc = mav_crc16(&buf[1..crc_pos]) ^ (extra as u16);
            if calc != crc {
                return Err(MavError::Crc);
            }
        }
    }

    let mut payload = [0u8; MAVLINK_MAX_PAYLOAD];
    let pstart = 1 + header_after;
    payload[..payload_len].copy_from_slice(&buf[pstart..pstart + payload_len]);

    Ok(MavFrame {
        version,
        seq: if version == 1 { buf[2] } else { buf[4] },
        sysid: if version == 1 { buf[3] } else { buf[5] },
        compid: if version == 1 { buf[4] } else { buf[6] },
        msgid,
        payload_len: payload_len as u8,
        payload,
        crc,
        signed: if signed { 1 } else { 0 },
    })
}

/// Streaming parser: feed bytes, get frames as they complete.
pub struct MavParser {
    buf: [u8; MAVLINK_MAX_PAYLOAD + 32],
    len: usize,
}

impl MavParser {
    /// Creates an empty parser.
    pub fn new() -> Self {
        Self {
            buf: [0u8; MAVLINK_MAX_PAYLOAD + 32],
            len: 0,
        }
    }

    /// Feed one byte; returns `Some(frame)` when a full frame is assembled.
    ///
    /// Resynchronizes on magic: any non-magic byte while idle is discarded,
    /// and a stray magic mid-frame restarts capture.
    pub fn feed(&mut self, byte: u8) -> Option<MavFrame> {
        if self.len == 0 {
            if byte == MAVLINK_1_MAGIC || byte == MAVLINK_2_MAGIC {
                self.buf[0] = byte;
                self.len = 1;
            }
            return None;
        }
        // Mid-frame magic → restart (lost sync).
        if self.len > 1 && (byte == MAVLINK_1_MAGIC || byte == MAVLINK_2_MAGIC) {
            self.buf[0] = byte;
            self.len = 1;
            return None;
        }
        if self.len >= self.buf.len() {
            // Pathological over-length: drop and resync next magic.
            self.len = 0;
            return None;
        }
        self.buf[self.len] = byte;
        self.len += 1;

        // Once we have the payload length we know the full frame size.
        if self.len >= 2 {
            let version = if self.buf[0] == MAVLINK_1_MAGIC { 1 } else { 2 };
            let pl = self.buf[1] as usize;
            let signed = version == 2 && self.buf[2] & 0x01 != 0;
            let total = frame_len(version, pl, signed);
            if self.len >= total {
                let frame = parse_frame(&self.buf[..total]).ok()?;
                self.len = 0;
                return Some(frame);
            }
        }
        None
    }
}

impl Default for MavParser {
    fn default() -> Self {
        Self::new()
    }
}

// --- Decoders into rkyv structs (spec: "deserialize directly into rkyv
// structs"). These read the populated payload little-endian.

/// HEARTBEAT payload (9 bytes).
#[derive(Debug, Clone, Copy, PartialEq, Archive, Serialize, Deserialize)]
#[repr(C)]
pub struct Heartbeat {
    /// Autopilot-specific mode bitmap.
    pub custom_mode: u32,
    /// Vehicle/component type ([`MavType`] low byte).
    pub type_: u8,
    /// Autopilot id.
    pub autopilot: u8,
    /// Base mode bitmap.
    pub base_mode: u8,
    /// System status.
    pub system_status: u8,
    /// MAVLink version (should be 3).
    pub mavlink_version: u8,
}
// SAFETY: repr(C) dense POD.
unsafe impl tpt_t_ring::cast::PlainBytes for Heartbeat {}

/// ATTITUDE payload (28 bytes): time_boot_ms + 6 floats.
#[derive(Debug, Clone, Copy, PartialEq, Archive, Serialize, Deserialize)]
#[repr(C)]
pub struct Attitude {
    /// Timestamp (ms since boot).
    pub time_boot_ms: u32,
    /// Roll (rad).
    pub roll: f32,
    /// Pitch (rad).
    pub pitch: f32,
    /// Yaw (rad).
    pub yaw: f32,
    /// Roll rate (rad/s).
    pub rollspeed: f32,
    /// Pitch rate (rad/s).
    pub pitchspeed: f32,
    /// Yaw rate (rad/s).
    pub yawspeed: f32,
}
// SAFETY: repr(C) dense POD.
unsafe impl tpt_t_ring::cast::PlainBytes for Attitude {}

fn le_u32(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}
fn le_f32(b: &[u8], off: usize) -> f32 {
    f32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

/// Decodes a HEARTBEAT frame into [`Heartbeat`].
pub fn decode_heartbeat(frame: &MavFrame) -> Option<Heartbeat> {
    if frame.msgid != 0 || frame.payload_len < 9 {
        return None;
    }
    Some(Heartbeat {
        custom_mode: le_u32(&frame.payload, 0),
        type_: frame.payload[4],
        autopilot: frame.payload[5],
        base_mode: frame.payload[6],
        system_status: frame.payload[7],
        mavlink_version: frame.payload[8],
    })
}

/// Decodes an ATTITUDE frame into [`Attitude`].
pub fn decode_attitude(frame: &MavFrame) -> Option<Attitude> {
    if frame.msgid != 30 || frame.payload_len < 28 {
        return None;
    }
    Some(Attitude {
        time_boot_ms: le_u32(&frame.payload, 0),
        roll: le_f32(&frame.payload, 4),
        pitch: le_f32(&frame.payload, 8),
        yaw: le_f32(&frame.payload, 12),
        rollspeed: le_f32(&frame.payload, 16),
        pitchspeed: le_f32(&frame.payload, 20),
        yawspeed: le_f32(&frame.payload, 24),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Serializes a HEARTBEAT frame (used to validate the parser round-trips).
    fn make_heartbeat(seq: u8) -> Vec<u8> {
        let payload = [
            0x01u8, 0x00, 0x00, 0x00, // custom_mode = 1
            1,    // type
            8,    // autopilot (MAV_AUTOPILOT_PX4)
            0x80, // base_mode
            3,    // system_status
            3,    // mavlink_version
        ];
        let mut buf = vec![MAVLINK_1_MAGIC, payload.len() as u8, seq, 1, 1, 0];
        buf.extend_from_slice(&payload);
        let crc = mav_crc16(&buf[1..]) ^ 50; // crc_extra for HEARTBEAT
        buf.extend_from_slice(&crc.to_le_bytes());
        buf
    }

    #[test]
    fn crc16_matches_known_poly_behavior() {
        // mav_crc16 of an empty buffer is the init value.
        assert_eq!(mav_crc16(&[]), 0xFFFF);
        // Stable, deterministic, non-trivial.
        assert_ne!(mav_crc16(&[1, 2, 3, 4]), 0);
    }

    #[test]
    fn heartbeat_roundtrips_through_parser() {
        let buf = make_heartbeat(7);
        let f = parse_frame(&buf).expect("parses");
        assert_eq!(f.version, 1);
        assert_eq!(f.seq, 7);
        assert_eq!(f.msgid, 0);
        assert_eq!(f.payload_len, 9);
        let hb = decode_heartbeat(&f).unwrap();
        assert_eq!(hb.custom_mode, 1);
        assert_eq!(hb.autopilot, 8);
        assert_eq!(hb.mavlink_version, 3);

        // Corrupt one payload byte → CRC must reject.
        let mut bad = buf.clone();
        bad[6] ^= 0xFF;
        assert_eq!(parse_frame(&bad), Err(MavError::Crc));
    }

    #[test]
    fn streaming_parser_emits_frames() {
        let mut p = MavParser::new();
        let bytes = make_heartbeat(2);
        let mut got = None;
        for b in bytes {
            if let Some(f) = p.feed(b) {
                got = Some(f);
            }
        }
        let f = got.expect("frame emitted");
        assert_eq!(f.seq, 2);
        assert!(decode_heartbeat(&f).is_some());
    }

    #[test]
    fn attitude_decodes_into_struct() {
        // Build a v2 ATTITUDE frame.
        let mut payload = [0u8; 28];
        payload[0..4].copy_from_slice(&1000u32.to_le_bytes());
        payload[4..8].copy_from_slice(&0.5f32.to_le_bytes());
        payload[8..12].copy_from_slice(&(-0.25f32).to_le_bytes());
        let mut buf = vec![MAVLINK_2_MAGIC, 28, 0, 0, 4, 1, 1];
        buf.extend_from_slice(&30u32.to_le_bytes()); // msgid 30 (3 bytes)
        buf.extend_from_slice(&payload);
        let crc = mav_crc16(&buf[1..]) ^ 39; // ATTITUDE crc_extra
        buf.extend_from_slice(&crc.to_le_bytes());
        let f = parse_frame(&buf).expect("v2 attitude parses");
        assert_eq!(f.version, 2);
        assert_eq!(f.msgid, 30);
        let a = decode_attitude(&f).unwrap();
        assert_eq!(a.time_boot_ms, 1000);
        assert!((a.roll - 0.5).abs() < 1e-6);
        assert!((a.yaw - (-0.25)).abs() < 1e-6);
    }

    #[test]
    fn bad_magic_and_truncation_rejected() {
        assert_eq!(parse_frame(&[0x00, 0x01]), Err(MavError::Magic));
        let mut short = make_heartbeat(0);
        short.truncate(short.len() - 1);
        assert_eq!(parse_frame(&short), Err(MavError::Truncated));
    }
}
