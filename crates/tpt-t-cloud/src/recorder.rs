//! Session recording — raw rkyv byte streams written to disk.
//!
//! Each DTI session records the verbatim wire payloads it ingests (control,
//! telemetry, media) so sessions can be replayed or audited offline. Frames
//! are written as a tiny self-describing container: a one-time file header
//! followed by per-frame `[channel][seq][len][payload]` records. No
//! intermediate allocation and no compression — the bytes on disk are exactly
//! the rkyv payloads the link layer produced.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use tpt_t_link::mux::MAX_PAYLOAD;

/// Magic marking a tpt-teleop session-recording file (`"TPTREC\0\0"`).
pub const REC_MAGIC: u64 = 0x5450_5452_4543_0000;
/// Recording container format revision.
pub const REC_VERSION: u16 = 1;

const HEADER_LEN: usize = 18;

/// A sink that persists raw frames for a session.
pub trait Recorder {
    /// Appends one frame (raw payload bytes) with its channel tag and sequence.
    fn record(&mut self, channel: u8, seq: u64, payload: &[u8]) -> io::Result<()>;

    /// Number of frames recorded so far.
    fn frames(&self) -> u64;
}

/// A recorder that discards everything (used when recording is disabled).
#[derive(Debug, Default, Clone)]
pub struct NullRecorder {
    count: u64,
}

impl NullRecorder {
    /// A no-op recorder.
    pub fn new() -> Self {
        Self { count: 0 }
    }
}

impl Recorder for NullRecorder {
    fn record(&mut self, _channel: u8, _seq: u64, _payload: &[u8]) -> io::Result<()> {
        self.count += 1;
        Ok(())
    }
    fn frames(&self) -> u64 {
        self.count
    }
}

/// An in-memory recorder (tests, debugging, or short ring-buffer captures).
#[derive(Debug, Default, Clone)]
pub struct VecRecorder {
    /// Recorded frames as `(channel, seq, payload)`.
    pub frames: Vec<(u8, u64, Vec<u8>)>,
}

impl VecRecorder {
    /// An empty in-memory recorder.
    pub fn new() -> Self {
        Self { frames: Vec::new() }
    }
}

impl Recorder for VecRecorder {
    fn record(&mut self, channel: u8, seq: u64, payload: &[u8]) -> io::Result<()> {
        self.frames.push((channel, seq, payload.to_vec()));
        Ok(())
    }
    fn frames(&self) -> u64 {
        self.frames.len() as u64
    }
}

/// A disk-backed recorder writing to `<dir>/unit-<id>.tptr`.
pub struct FileRecorder {
    file: File,
    count: u64,
}

impl FileRecorder {
    /// Opens (creating/append) a recording file for `unit_id` under `dir`.
    pub fn create(dir: &Path, unit_id: u64) -> io::Result<Self> {
        std::fs::create_dir_all(dir)?;
        let path = dir.join(format!("unit-{unit_id}.tptr"));
        let mut file = OpenOptions::new()
            .create(true)
            .read(true)
            .append(true)
            .open(&path)?;
        // Write the header only when the file is new/empty.
        if file.metadata()?.len() == 0 {
            let mut hdr = [0u8; HEADER_LEN];
            hdr[0..8].copy_from_slice(&REC_MAGIC.to_le_bytes());
            hdr[8..10].copy_from_slice(&REC_VERSION.to_le_bytes());
            hdr[10..18].copy_from_slice(&unit_id.to_le_bytes());
            file.write_all(&hdr)?;
            file.flush()?;
        }
        Ok(Self { file, count: 0 })
    }

    /// Flushes buffered data to disk. The OS also flushes on drop.
    pub fn close(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

impl Recorder for FileRecorder {
    fn record(&mut self, channel: u8, seq: u64, payload: &[u8]) -> io::Result<()> {
        if payload.len() > MAX_PAYLOAD {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "payload exceeds recording MTU",
            ));
        }
        let mut frame = [0u8; 13];
        frame[0] = channel;
        frame[1..9].copy_from_slice(&seq.to_le_bytes());
        frame[9..13].copy_from_slice(&(payload.len() as u32).to_le_bytes());
        self.file.write_all(&frame)?;
        self.file.write_all(payload)?;
        self.count += 1;
        Ok(())
    }
    fn frames(&self) -> u64 {
        self.count
    }
}

impl Drop for FileRecorder {
    fn drop(&mut self) {
        let _ = self.file.flush();
    }
}

/// One decoded recording frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedFrame {
    /// Channel tag.
    pub channel: u8,
    /// Sequence number.
    pub seq: u64,
    /// Raw payload bytes.
    pub payload: Vec<u8>,
}

/// Reads every frame back from a recording file.
pub fn read_frames(path: &Path) -> io::Result<Vec<RecordedFrame>> {
    let data = std::fs::read(path)?;
    if data.len() < HEADER_LEN {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "file too short"));
    }
    let magic = u64::from_le_bytes(data[0..8].try_into().unwrap());
    if magic != REC_MAGIC {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "bad recording magic",
        ));
    }
    let mut out = Vec::new();
    let mut i = HEADER_LEN;
    while i + 13 <= data.len() {
        let channel = data[i];
        let seq = u64::from_le_bytes(data[i + 1..i + 9].try_into().unwrap());
        let len = u32::from_le_bytes(data[i + 9..i + 13].try_into().unwrap()) as usize;
        i += 13;
        if i + len > data.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "truncated frame",
            ));
        }
        out.push(RecordedFrame {
            channel,
            seq,
            payload: data[i..i + len].to_vec(),
        });
        i += len;
    }
    Ok(out)
}

/// Convenience: create a recorder file in a temp-style directory. Used by
/// tests and by the fleet's on-disk provisioning path.
pub fn temp_recorder_dir(name: &str) -> PathBuf {
    let mut dir = std::env::temp_dir();
    dir.push("tpt-teleop");
    dir.push(format!(
        "{name}-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    dir
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn null_recorder_counts() {
        let mut r = NullRecorder::new();
        r.record(1, 7, b"abc").unwrap();
        r.record(2, 8, b"defg").unwrap();
        assert_eq!(r.frames(), 2);
    }

    #[test]
    fn vec_recorder_roundtrips() {
        let mut r = VecRecorder::new();
        r.record(3, 11, b"payload").unwrap();
        assert_eq!(r.frames(), 1);
        assert_eq!(r.frames[0], (3, 11, b"payload".to_vec()));
    }

    #[test]
    fn file_recorder_writes_and_reads_back() {
        let dir = temp_recorder_dir("rec-test");
        let path = dir.join("unit-99.tptr");
        let _ = std::fs::remove_file(&path);
        {
            let mut r = FileRecorder::create(&dir, 99).unwrap();
            r.record(1, 1, b"alpha").unwrap();
            r.record(2, 42, b"beta-beta").unwrap();
            r.close().unwrap();
        }
        let frames = read_frames(&path).unwrap();
        assert_eq!(frames.len(), 2);
        assert_eq!(
            frames[0],
            RecordedFrame {
                channel: 1,
                seq: 1,
                payload: b"alpha".to_vec()
            }
        );
        assert_eq!(
            frames[1],
            RecordedFrame {
                channel: 2,
                seq: 42,
                payload: b"beta-beta".to_vec()
            }
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn file_recorder_rejects_oversize() {
        let dir = temp_recorder_dir("rec-big");
        let mut r = FileRecorder::create(&dir, 1).unwrap();
        let big = vec![0u8; MAX_PAYLOAD + 1];
        assert!(r.record(3, 0, &big).is_err());
    }
}
