//! Camera ingestion interface (zero-copy grab into caller-owned buffers).

use crate::types::{FrameInfo, HalError, PixelFormat};

/// Frame-grabbing camera.
///
/// The buffer is supplied by the caller (typically a slab from
/// `tpt-teleop-core::pool`) and filled in place — the driver never copies
/// into intermediate storage. Real V4L2/DirectShow/Metal backends land with
/// Phase 8; `mock::camera::NullCamera` serves tests until then.
pub trait Camera: Send {
    /// Configures capture geometry. Reconfiguring mid-stream stops it.
    fn configure(&mut self, width: u32, height: u32, format: PixelFormat) -> Result<(), HalError>;

    /// Grabs one frame into `buf`, filling `info`. Returns
    /// [`HalError::BufferTooSmall`] if `buf.len()` is below the format's
    /// minimum for the configured geometry.
    fn grab(&mut self, buf: &mut [u8], info: &mut FrameInfo) -> Result<(), HalError>;
}
