//! FDR record framing and the wait-free ring handoff between the control loop
//! (producer) and the storage thread (consumer). See `crate` docs for the
//! end-to-end design.

use std::io;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use tpt_t_core::ser::{ControlCommand, GpsSample, ImuSample, TelemetryPacket};
use tpt_t_ring::SpscRing;
use tpt_t_ring::cast::{self, PlainBytes};

use crate::direct_io::DirectFile;

/// Maximum payload bytes stored inline in an [`FdrEntry`]. Large enough for
/// every current wire struct (largest is [`GpsSample`] at 64 bytes) with ample
/// headroom for rkyv-archived variants.
pub const MAX_PAYLOAD: usize = 512;

/// What kind of sample an [`FdrEntry`] carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum RecordKind {
    /// [`ControlCommand`].
    Control = 0,
    /// [`ImuSample`].
    Imu = 1,
    /// [`GpsSample`].
    Gps = 2,
    /// [`TelemetryPacket`].
    Telemetry = 3,
    /// Internal end-of-stream marker written by the writer on close so readers
    /// stop before the zero-padding that direct I/O appends to the final
    /// sector. Never produced by the hot path.
    End = 255,
}

impl RecordKind {
    /// Inverse of the `repr(u8)` discriminant.
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Control),
            1 => Some(Self::Imu),
            2 => Some(Self::Gps),
            3 => Some(Self::Telemetry),
            255 => Some(Self::End),
            _ => None,
        }
    }
}

/// Fixed-layout FDR frame: a small header plus an inline payload. The whole
/// struct is `#[repr(C)]` plain-old-data with no padding, so it is both
/// rkyv-castable and blittable straight to disk.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct FdrEntry {
    /// Capture timestamp (UNIX ns).
    pub timestamp_ns: u64,
    /// Payload byte count following the header.
    pub payload_len: u16,
    /// [`RecordKind`] discriminant.
    pub kind: u8,
    /// Reserved.
    pub _pad: u8,
    /// Reserved (keeps the header a 16-byte, 8-aligned block so the trailing
    /// `[u8; MAX_PAYLOAD]` needs no implicit padding for `PlainBytes`).
    pub _reserved: u32,
    /// Inline payload (only `payload_len` bytes are meaningful).
    pub payload: [u8; MAX_PAYLOAD],
}

// SAFETY: repr(C) with an 8-byte-aligned u64 header, an explicit 4-byte
// reserved word, and a `[u8; MAX_PAYLOAD]` (MAX_PAYLOAD is a multiple of 8),
// so the struct has no interior or tail padding. Every bit pattern of these
// fields is a valid value.
unsafe impl PlainBytes for FdrEntry {}

impl FdrEntry {
    /// Builds a frame from `payload` (must fit in [`MAX_PAYLOAD`]).
    pub fn new(kind: RecordKind, payload: &[u8], timestamp_ns: u64) -> Result<Self, RecordError> {
        if payload.len() > MAX_PAYLOAD {
            return Err(RecordError::PayloadTooLarge {
                got: payload.len(),
                max: MAX_PAYLOAD,
            });
        }
        let mut e = Self {
            timestamp_ns,
            payload_len: payload.len() as u16,
            kind: kind as u8,
            _pad: 0,
            _reserved: 0,
            payload: [0u8; MAX_PAYLOAD],
        };
        e.payload[..payload.len()].copy_from_slice(payload);
        Ok(e)
    }

    /// The kind, or `None` for an unrecognized discriminant.
    pub fn kind(&self) -> Option<RecordKind> {
        RecordKind::from_u8(self.kind)
    }

    /// The meaningful payload slice.
    pub fn payload(&self) -> &[u8] {
        &self.payload[..self.payload_len as usize]
    }
}

/// Errors returned by the FDR producer API.
#[derive(Debug)]
pub enum RecordError {
    /// The ring was full — the record was shed rather than block the loop.
    Full,
    /// Payload exceeded [`MAX_PAYLOAD`].
    PayloadTooLarge { got: usize, max: usize },
    /// Underlying I/O failure on the storage thread.
    Io(io::Error),
}

impl From<io::Error> for RecordError {
    fn from(e: io::Error) -> Self {
        RecordError::Io(e)
    }
}

impl std::fmt::Display for RecordError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RecordError::Full => write!(f, "FDR ring full: record dropped (non-blocking)"),
            RecordError::PayloadTooLarge { got, max } => {
                write!(f, "payload {got} bytes exceeds MAX_PAYLOAD {max}")
            }
            RecordError::Io(e) => write!(f, "FDR I/O error: {e}"),
        }
    }
}

impl std::error::Error for RecordError {}

/// Reconstructs a `PlainBytes` value from an arbitrary (possibly
/// unaligned) byte slice by copying into an aligned local. Used by the
/// offline reader/export paths where alignment is not guaranteed.
pub fn from_bytes_aligned<T: PlainBytes + Copy>(data: &[u8]) -> Option<T> {
    if data.len() < std::mem::size_of::<T>() {
        return None;
    }
    let mut slot = std::mem::MaybeUninit::<T>::uninit();
    // SAFETY: `slot` is aligned for `T` and at least `size_of::<T>()` bytes;
    // `T: PlainBytes` means every bit pattern is a valid value, so copying
    // raw bytes and `assume_init` is sound.
    unsafe {
        std::ptr::copy_nonoverlapping(
            data.as_ptr(),
            slot.as_mut_ptr() as *mut u8,
            std::mem::size_of::<T>(),
        );
        Some(slot.assume_init())
    }
}

/// Producer-side handle. Publishes records into the SPSC ring without ever
/// blocking (a full ring returns [`RecordError::Full`] immediately).
pub struct FdrSink {
    ring: Arc<SpscRing<FdrEntry>>,
}

impl FdrSink {
    /// Wraps the producer half of `ring`.
    pub fn new(ring: Arc<SpscRing<FdrEntry>>) -> Self {
        Self { ring }
    }

    /// Publishes `payload` of `kind` stamped at `timestamp_ns`.
    ///
    /// Returns [`RecordError::Full`] if the ring is full — the caller sheds the
    /// record rather than stalling the control loop.
    pub fn try_record(
        &self,
        kind: RecordKind,
        payload: &[u8],
        timestamp_ns: u64,
    ) -> Result<(), RecordError> {
        let e = FdrEntry::new(kind, payload, timestamp_ns)?;
        self.ring.push(e).map_err(|_| RecordError::Full)
    }

    /// Convenience: logs a [`ControlCommand`] (its wire bytes are the rkyv
    /// archived form for these POD structs).
    pub fn try_control(&self, c: &ControlCommand) -> Result<(), RecordError> {
        self.try_record(RecordKind::Control, cast::bytes_of(c), c.timestamp_ns)
    }

    /// Convenience: logs an [`ImuSample`].
    pub fn try_imu(&self, s: &ImuSample) -> Result<(), RecordError> {
        self.try_record(RecordKind::Imu, cast::bytes_of(s), s.timestamp_ns)
    }

    /// Convenience: logs a [`GpsSample`].
    pub fn try_gps(&self, s: &GpsSample) -> Result<(), RecordError> {
        self.try_record(RecordKind::Gps, cast::bytes_of(s), s.timestamp_ns)
    }

    /// Convenience: logs a [`TelemetryPacket`].
    pub fn try_telemetry(&self, s: &TelemetryPacket) -> Result<(), RecordError> {
        self.try_record(RecordKind::Telemetry, cast::bytes_of(s), s.timestamp_ns)
    }

    /// True when a subsequent [`try_record`](Self::try_record) would shed.
    pub fn is_full(&self) -> bool {
        self.ring.is_full()
    }

    /// Number of frames currently buffered for the storage thread.
    pub fn pending(&self) -> usize {
        self.ring.len()
    }
}

/// Consumer-side writer. Drains the ring and writes frames (plus an end
/// marker) to a [`DirectFile`]. Intended to run on the dedicated storage
/// thread.
pub struct FdrWriter {
    ring: Arc<SpscRing<FdrEntry>>,
    file: DirectFile,
}

impl FdrWriter {
    /// Opens the writer against `ring`, creating/truncating `path` for
    /// direct-I/O writing.
    pub fn open(ring: Arc<SpscRing<FdrEntry>>, path: &Path) -> io::Result<Self> {
        Ok(Self {
            ring,
            file: DirectFile::open(path)?,
        })
    }

    /// Whether the underlying file is using true OS direct I/O.
    pub fn is_direct(&self) -> bool {
        self.file.is_direct()
    }

    /// Bytes written to the kernel so far (sector-multiple writes only).
    pub fn written(&self) -> u64 {
        self.file.written()
    }

    /// Drains every currently-available frame into the file. Returns how many
    /// frames were written.
    pub fn drain_once(&mut self) -> io::Result<usize> {
        let mut n = 0;
        while let Some(e) = self.ring.pop() {
            // SAFETY: bytes_of yields a shared view of the POD FdrEntry; the
            // slice length equals size_of, so the write is in-bounds.
            let bytes = cast::bytes_of(&e);
            self.file.write(bytes)?;
            n += 1;
        }
        Ok(n)
    }

    /// Runs until `stop` is set, then flushes an end marker and the file.
    /// Returns total bytes written.
    pub fn run(mut self, stop: Arc<AtomicBool>) -> io::Result<u64> {
        loop {
            let n = self.drain_once()?;
            if n == 0 {
                if stop.load(Ordering::Acquire) {
                    break;
                }
                thread::sleep(Duration::from_micros(250));
            }
        }
        let end = FdrEntry::new(RecordKind::End, &[], 0).expect("empty payload is always valid");
        self.file.write(cast::bytes_of(&end))?;
        self.file.flush()?;
        Ok(self.file.written())
    }
}

/// Standalone recorder: owns the ring, spawns the storage thread, and exposes
/// a [`FdrSink`] for the hot path to publish into. Drop (or
/// [`Recorder::stop_and_join`]) stops the thread and flushes the file.
pub struct Recorder {
    sink: FdrSink,
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<io::Result<u64>>>,
    path: std::path::PathBuf,
}

impl Recorder {
    /// Opens `path` and starts the storage thread with a ring of
    /// `ring_capacity` frames.
    pub fn open(path: impl AsRef<Path>, ring_capacity: usize) -> io::Result<Self> {
        let ring = Arc::new(SpscRing::<FdrEntry>::with_capacity(ring_capacity));
        let writer = FdrWriter::open(ring.clone(), path.as_ref())?;
        let stop = Arc::new(AtomicBool::new(false));
        let stop_th = Arc::clone(&stop);
        let join = thread::spawn(move || writer.run(stop_th));
        Ok(Self {
            sink: FdrSink::new(ring),
            stop,
            join: Some(join),
            path: path.as_ref().to_path_buf(),
        })
    }

    /// Producer handle for the control loop.
    pub fn sink(&self) -> &FdrSink {
        &self.sink
    }

    /// Path being written.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Signals the storage thread to stop and waits for it to flush.
    pub fn stop_and_join(mut self) -> io::Result<u64> {
        self.stop.store(true, Ordering::Release);
        self.join
            .take()
            .expect("join handle present")
            .join()
            .expect("storage thread panicked")
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        if let Some(j) = self.join.take() {
            self.stop.store(true, Ordering::Release);
            let _ = j.join();
        }
    }
}

/// Parses a raw FDR byte stream back into frames, stopping at the end marker
/// or on truncation. Used by tests and by offline tooling.
pub fn parse_entries(data: &[u8]) -> Vec<FdrEntry> {
    let mut out = Vec::new();
    let sz = std::mem::size_of::<FdrEntry>();
    let mut off = 0;
    while off + sz <= data.len() {
        let Some(e) = from_bytes_aligned::<FdrEntry>(&data[off..off + sz]) else {
            break;
        };
        if e.kind() == Some(RecordKind::End) {
            break;
        }
        out.push(e);
        off += sz;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use tpt_t_core::mode::Mode;

    #[test]
    fn fdr_entry_layout_has_no_padding() {
        // header: u64(8) + u16(2) + u8 + u8 + u32(4) = 16, then [u8;512] -> 528.
        // MAX_PAYLOAD (512) is a multiple of 8 so no trailing padding exists.
        assert_eq!(std::mem::size_of::<FdrEntry>(), 16 + MAX_PAYLOAD);
        let e = FdrEntry::new(RecordKind::Control, &[1, 2, 3], 42).unwrap();
        assert_eq!(e.kind(), Some(RecordKind::Control));
        assert_eq!(e.payload(), &[1, 2, 3]);
        assert_eq!(e.timestamp_ns, 42);
    }

    #[test]
    fn payload_too_large_is_rejected() {
        let big = vec![0u8; MAX_PAYLOAD + 1];
        assert!(matches!(
            FdrEntry::new(RecordKind::Control, &big, 0),
            Err(RecordError::PayloadTooLarge { .. })
        ));
    }

    #[test]
    fn full_ring_sheds_without_blocking() {
        let ring = Arc::new(SpscRing::<FdrEntry>::with_capacity(2));
        let sink = FdrSink::new(ring);
        let cmd = ControlCommand::zeroed(Mode::FullTeleop);
        assert!(sink.try_control(&cmd).is_ok());
        assert!(sink.try_control(&cmd).is_ok());
        // Third push must fail fast, never block.
        assert!(matches!(sink.try_control(&cmd), Err(RecordError::Full)));
    }

    #[test]
    fn recorder_round_trips_frames() {
        let dir = std::env::temp_dir().join("tpt_fdr_test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join(format!("fdr_{}.bin", std::process::id()));

        let rec = Recorder::open(&path, 64).unwrap();
        for i in 0..10u64 {
            let cmd = ControlCommand::zeroed(Mode::FullTeleop);
            rec.sink().try_control(&cmd).unwrap();
            let imu = ImuSample {
                gyro_rps: [i as f32, 0.0, 0.0],
                ..ImuSample::zeroed(i, i * 100)
            };
            rec.sink().try_imu(&imu).unwrap();
        }
        let written = rec.stop_and_join().unwrap();
        assert!(written > 0);

        let bytes = std::fs::read(&path).unwrap();
        let entries = parse_entries(&bytes);
        // 10 control + 10 imu = 20 frames.
        assert_eq!(entries.len(), 20);
        let controls = entries
            .iter()
            .filter(|e| e.kind() == Some(RecordKind::Control))
            .count();
        let imus = entries
            .iter()
            .filter(|e| e.kind() == Some(RecordKind::Imu))
            .count();
        assert_eq!(controls, 10);
        assert_eq!(imus, 10);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn end_marker_stops_parse_before_padding() {
        // Build a stream: one real frame + a deliberately longer run of
        // zero padding (as direct I/O would append). parse_entries must stop
        // at the End marker and not treat trailing zeros as a frame.
        let mut stream = Vec::new();
        let real = FdrEntry::new(RecordKind::Control, &[9, 8, 7], 1).unwrap();
        stream.extend_from_slice(cast::bytes_of(&real));
        let end = FdrEntry::new(RecordKind::End, &[], 0).unwrap();
        stream.extend_from_slice(cast::bytes_of(&end));
        stream.extend(std::iter::repeat_n(0u8, sector_padding()));

        let parsed = parse_entries(&stream);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].payload(), &[9, 8, 7]);
    }

    fn sector_padding() -> usize {
        crate::direct_io::SECTOR_SIZE
    }
}
