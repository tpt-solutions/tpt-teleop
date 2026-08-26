//! Camera capture backends (spec §5.3 "zero-copy capture backend:
//! Linux V4L2 / Windows DirectShow / macOS").
//!
//! [`CaptureBackend`] mirrors the HAL [`Camera`](tpt_t_hal::camera::Camera)
//! contract: the caller supplies the destination buffer (typically a slab
//! block from [`crate::pool`]) and the driver fills it in place — no
//! intermediate copy.
//!
//! The real V4L2 / DirectShow / AVFoundation backends are deferred to
//! hardware bring-up (their FFI surfaces are large and, for some, behind
//! non-MIT-chain bindings); until then each opens with
//! [`MediaError::Unsupported`] so a session fails loudly rather than silently
//! grabbing nothing. [`TestPatternCapture`] is the zero-dependency source
//! used by the simulator, integration tests, and dev rigs.

use crate::MediaError;
use crate::pool::{FrameMeta, PixFmt};

/// A frame-grabbing capture source.
pub trait CaptureBackend {
    /// Configures geometry/format. Reconfiguring mid-stream is allowed.
    fn configure(&mut self, width: u32, height: u32, format: PixFmt) -> Result<(), MediaError>;

    /// Fills `buf` with one frame and writes its [`FrameMeta`]. Returns
    /// [`MediaError::BufferTooSmall`] if `buf` is shorter than the format's
    /// minimum for the configured geometry.
    fn grab(&mut self, buf: &mut [u8], meta: &mut FrameMeta) -> Result<(), MediaError>;
}

/// Software source emitting a deterministic gradient — the simulator's camera.
/// Each grab advances an internal sequence and timestamp so downstream
/// burn-in / encode see a moving picture.
#[derive(Debug)]
pub struct TestPatternCapture {
    width: u32,
    height: u32,
    format: PixFmt,
    seq: u64,
    start_ns: u64,
}

impl TestPatternCapture {
    /// Creates a source for `width×height` in `format`.
    pub fn new(width: u32, height: u32, format: PixFmt) -> Self {
        Self {
            width,
            height,
            format,
            seq: 0,
            start_ns: 0,
        }
    }

    fn paint(&self, buf: &mut [u8]) {
        let (w, h) = (self.width as usize, self.height as usize);
        match self.format {
            PixFmt::Rgb888 => {
                let stride = 3 * w;
                for y in 0..h {
                    for x in 0..w {
                        let i = y * stride + x * 3;
                        buf[i] = (x % 256) as u8;
                        buf[i + 1] = (y % 256) as u8;
                        buf[i + 2] = ((x + y) % 256) as u8;
                    }
                }
            }
            PixFmt::GrayY8 => {
                for y in 0..h {
                    for x in 0..w {
                        buf[y * w + x] = ((x + y) % 256) as u8;
                    }
                }
            }
            PixFmt::Nv12 => {
                let y_plane = w * h;
                for y in 0..h {
                    for x in 0..w {
                        buf[y * w + x] = ((x + y) % 256) as u8;
                    }
                }
                for b in &mut buf[y_plane..] {
                    *b = 128; // neutral chroma
                }
            }
        }
    }
}

impl CaptureBackend for TestPatternCapture {
    fn configure(&mut self, width: u32, height: u32, format: PixFmt) -> Result<(), MediaError> {
        self.width = width;
        self.height = height;
        self.format = format;
        Ok(())
    }

    fn grab(&mut self, buf: &mut [u8], meta: &mut FrameMeta) -> Result<(), MediaError> {
        let need = self.format.min_buffer_len(self.width, self.height);
        if buf.len() < need {
            return Err(MediaError::BufferTooSmall {
                needed: need,
                got: buf.len(),
            });
        }
        self.paint(buf);
        self.seq = self.seq.wrapping_add(1);
        *meta = FrameMeta::new(
            self.seq,
            self.start_ns + self.seq * 33_333_333,
            self.width,
            self.height,
            self.format,
        );
        Ok(())
    }
}

/// Linux V4L2 capture backend.
///
/// Deferred: V4L2 `ioctl` plumbing (mmap ring, format negotiation) is bound at
/// hardware bring-up. Construction fails loudly meanwhile.
#[derive(Debug, Default)]
pub struct V4l2Capture {
    _private: (),
}

impl V4l2Capture {
    /// Always [`MediaError::Unsupported`] until the V4L2 binding lands.
    pub fn open(_device: &str) -> Result<Self, MediaError> {
        Err(MediaError::Unsupported(
            "V4L2 binding deferred to hardware bring-up",
        ))
    }
}

impl CaptureBackend for V4l2Capture {
    fn configure(&mut self, _w: u32, _h: u32, _f: PixFmt) -> Result<(), MediaError> {
        Err(MediaError::Unsupported("V4L2 binding deferred"))
    }
    fn grab(&mut self, _buf: &mut [u8], _meta: &mut FrameMeta) -> Result<(), MediaError> {
        Err(MediaError::Unsupported("V4L2 binding deferred"))
    }
}

/// Windows DirectShow capture backend (deferred; see [`V4l2Capture`]).
#[derive(Debug, Default)]
pub struct DirectShowCapture {
    _private: (),
}

impl DirectShowCapture {
    /// Always [`MediaError::Unsupported`] until the DirectShow binding lands.
    pub fn open(_device: &str) -> Result<Self, MediaError> {
        Err(MediaError::Unsupported(
            "DirectShow binding deferred to hardware bring-up",
        ))
    }
}

impl CaptureBackend for DirectShowCapture {
    fn configure(&mut self, _w: u32, _h: u32, _f: PixFmt) -> Result<(), MediaError> {
        Err(MediaError::Unsupported("DirectShow binding deferred"))
    }
    fn grab(&mut self, _buf: &mut [u8], _meta: &mut FrameMeta) -> Result<(), MediaError> {
        Err(MediaError::Unsupported("DirectShow binding deferred"))
    }
}

/// macOS AVFoundation capture backend (deferred; see [`V4l2Capture`]).
#[derive(Debug, Default)]
pub struct MacCapture {
    _private: (),
}

impl MacCapture {
    /// Always [`MediaError::Unsupported`] until the AVFoundation binding lands.
    pub fn open(_device: &str) -> Result<Self, MediaError> {
        Err(MediaError::Unsupported(
            "AVFoundation binding deferred to hardware bring-up",
        ))
    }
}

impl CaptureBackend for MacCapture {
    fn configure(&mut self, _w: u32, _h: u32, _f: PixFmt) -> Result<(), MediaError> {
        Err(MediaError::Unsupported("AVFoundation binding deferred"))
    }
    fn grab(&mut self, _buf: &mut [u8], _meta: &mut FrameMeta) -> Result<(), MediaError> {
        Err(MediaError::Unsupported("AVFoundation binding deferred"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pattern_fills_buffer_and_advances() {
        let mut cam = TestPatternCapture::new(16, 8, PixFmt::Rgb888);
        let mut buf = vec![0u8; PixFmt::Rgb888.min_buffer_len(16, 8)];
        let mut m1 = FrameMeta::default();
        cam.grab(&mut buf, &mut m1).unwrap();
        assert_eq!(m1.width, 16);
        assert_eq!(m1.height, 8);
        assert_eq!(m1.seq, 1);
        // Gradient present (not all zero, not all identical).
        assert!(buf.iter().any(|&b| b != 0));
        let mut m2 = FrameMeta::default();
        cam.grab(&mut buf, &mut m2).unwrap();
        assert_eq!(m2.seq, 2);
        assert!(m2.timestamp_ns > m1.timestamp_ns);
    }

    #[test]
    fn test_pattern_rejects_tiny_buffer() {
        let mut cam = TestPatternCapture::new(64, 64, PixFmt::Rgb888);
        let mut buf = [0u8; 10];
        let mut meta = FrameMeta::default();
        assert!(matches!(
            cam.grab(&mut buf, &mut meta),
            Err(MediaError::BufferTooSmall { .. })
        ));
    }

    #[test]
    fn platform_backends_open_unsupported() {
        assert!(V4l2Capture::open("/dev/video0").is_err());
        assert!(DirectShowCapture::open("default").is_err());
        assert!(MacCapture::open("default").is_err());
    }
}
