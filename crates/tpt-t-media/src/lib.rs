//! Media & Telemetry layer (spec §5.3): everything between a camera and the
//! encoder that feeds the link.
//!
//! * [`pool`] — custom slab allocator / memory pool for video frames and
//!   sensor packets. One contiguous allocation, fixed-size blocks, O(1)
//!   alloc/free, zero heap traffic on the hot path.
//! * [`capture`] — capture backends. [`capture::TestPatternCapture`] is the
//!   zero-dependency simulator source; the real V4L2 / DirectShow /
//!   AVFoundation backends open with [`MediaError::Unsupported`] until
//!   hardware bring-up (their FFI is large and, for some, behind non-MIT-chain
//!   bindings).
//! * [`burnin`] — telemetry HUD rasterized directly into the pixel buffer
//!   before encode, using a built-in 5×7 font and zero intermediate storage.
//! * [`encoder`] — [`encoder::VideoEncoder`] trait plus a software
//!   [`encoder::NullEncoder`] for sim/tests, hardware stubs (NVENC/AMF), and
//!   [`encoder::EncoderGovernor`], which slews the encoder bitrate toward the
//!   Phase 7 [`tpt_t_link::backpressure::Backpressure`] signal.

pub mod burnin;
pub mod capture;
pub mod encoder;
pub mod pool;

/// Crate version (from Cargo metadata).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Media-layer failures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MediaError {
    /// Operation needs a hardware backend that is not bound yet.
    Unsupported(&'static str),
    /// Caller-provided buffer too small.
    BufferTooSmall {
        /// Bytes required.
        needed: usize,
        /// Bytes provided.
        got: usize,
    },
    /// Device rejected the operation; vendor text attached.
    Device(&'static str),
}

impl core::fmt::Display for MediaError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            MediaError::Unsupported(s) => write!(f, "unsupported: {s}"),
            MediaError::BufferTooSmall { needed, got } => {
                write!(f, "buffer too small: need {needed}, got {got}")
            }
            MediaError::Device(s) => write!(f, "device error: {s}"),
        }
    }
}

impl std::error::Error for MediaError {}

// Re-exports for ergonomic downstream use.
pub use pool::{FrameMeta, FramePool, PixFmt};
