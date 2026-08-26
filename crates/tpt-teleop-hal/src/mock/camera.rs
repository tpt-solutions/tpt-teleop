//! Null camera: deterministic test frames without hardware.

use crate::camera::Camera;
use crate::types::{FrameInfo, HalError, PixelFormat};

/// Generates flat gray frames of the configured geometry — enough to exercise
/// pipeline plumbing until real capture backends arrive (Phase 8).
#[derive(Debug)]
pub struct NullCamera {
    width: u32,
    height: u32,
    format: PixelFormat,
    seq: u64,
}

impl NullCamera {
    /// Unconfigured camera; call [`configure`](Camera::configure) first.
    pub fn new() -> Self {
        Self {
            width: 0,
            height: 0,
            format: PixelFormat::GrayY8,
            seq: 0,
        }
    }
}

impl Default for NullCamera {
    fn default() -> Self {
        Self::new()
    }
}

impl Camera for NullCamera {
    fn configure(&mut self, width: u32, height: u32, format: PixelFormat) -> Result<(), HalError> {
        self.width = width;
        self.height = height;
        self.format = format;
        Ok(())
    }

    fn grab(&mut self, buf: &mut [u8], info: &mut FrameInfo) -> Result<(), HalError> {
        if self.width == 0 {
            return Err(HalError::Device("not configured"));
        }
        let needed = self.format.min_buffer_len(self.width, self.height);
        if buf.len() < needed {
            return Err(HalError::BufferTooSmall {
                needed,
                got: buf.len(),
            });
        }
        // Deterministic ramp pattern so pipelines can detect corruption.
        for (i, b) in buf[..needed].iter_mut().enumerate() {
            *b = (i % 251) as u8;
        }
        self.seq += 1;
        *info = FrameInfo {
            seq: self.seq,
            timestamp_ns: self.seq * 33_333_333, // ~30 fps cadence
            width: self.width,
            height: self.height,
            stride: self.width * self.format.bytes_per_pixel(),
            format: self.format as u32,
        };
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn geometry_and_buffer_checks() {
        let mut cam = NullCamera::new();
        cam.configure(64, 32, PixelFormat::GrayY8).unwrap();

        let mut small = [0u8; 100];
        let mut info = FrameInfo::default();
        assert!(matches!(
            cam.grab(&mut small, &mut info),
            Err(HalError::BufferTooSmall { .. })
        ));

        let mut buf = vec![0u8; PixelFormat::GrayY8.min_buffer_len(64, 32)];
        cam.grab(&mut buf, &mut info).unwrap();
        assert_eq!(info.width, 64);
        assert_eq!(info.height, 32);
        assert_eq!(info.stride, 64);
        assert_eq!(info.format, PixelFormat::GrayY8 as u32);
        assert_eq!(info.seq, 1);
        assert_ne!(&buf[..], &[0u8; 2048], "pattern must not be all-zero");

        // NV12 sizing: w*h*3/2.
        cam.configure(640, 480, PixelFormat::Nv12).unwrap();
        assert_eq!(
            PixelFormat::Nv12.min_buffer_len(640, 480),
            640 * 480 * 3 / 2
        );
    }
}
