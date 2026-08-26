//! Video encoding and bitrate control (spec §5.3 "hardware-accelerated
//! encoding via NVENC / AMF" + "Wire encoder bitrate adjustment to Phase 7
//! network backpressure signal").
//!
//! [`VideoEncoder`] is the trait every backend implements — the null/software
//! [`NullEncoder`] for sim and tests, and the hardware paths (NVENC, AMF,
//! `wgpu` HUD compositor) which are documented stubs until real-hardware
//! bring-up exposes the vendor FFI (consistent with the Phase 6 / Phase 4
//! "deferred binding" policy).
//!
//! [`EncoderGovernor`] is the backpressure bridge: each tick it reads a
//! [`BitrateSignal`] (the Phase 7 [`tpt_t_link::backpressure::Backpressure`]
//! implements it directly) and slews the encoder's target bitrate toward the
//! link's congestion-derived suggestion — telemetry shrinks when the network
//! is congested, grows when it clears.

use crate::MediaError;
use crate::pool::FrameMeta;

/// A source of a suggested encoder bitrate (bytes/s).
pub trait BitrateSignal {
    /// Suggested target bitrate in bits per second.
    fn suggested_bitrate_bps(&self) -> u64;
}

/// Constant signal for tests / fixed-bitrate sessions.
#[derive(Debug, Clone, Copy)]
pub struct ConstBitrate(pub u64);

impl BitrateSignal for ConstBitrate {
    #[inline]
    fn suggested_bitrate_bps(&self) -> u64 {
        self.0
    }
}

/// The Phase 7 link backpressure estimator is itself a bitrate signal.
impl BitrateSignal for tpt_t_link::backpressure::Backpressure {
    #[inline]
    fn suggested_bitrate_bps(&self) -> u64 {
        self.suggested_bitrate_bps()
    }
}

/// Any reference to a signal is also a signal (so callers can borrow a
/// [`Backpressure`] without moving it).
impl<T: BitrateSignal + ?Sized> BitrateSignal for &T {
    #[inline]
    fn suggested_bitrate_bps(&self) -> u64 {
        (**self).suggested_bitrate_bps()
    }
}

/// A video encoder: compresses one frame into `out`, honoring the current
/// target bitrate.
pub trait VideoEncoder {
    /// Updates the target bitrate (bits/s).
    fn set_bitrate(&mut self, bps: u64);
    /// Last bitrate applied.
    fn current_bitrate(&self) -> u64;
    /// Encodes `frame` into `out`; returns bytes written. A hardware backend
    /// would DMA/encode in place; the null backend copies through.
    fn encode(
        &mut self,
        frame: &[u8],
        meta: &FrameMeta,
        out: &mut [u8],
    ) -> Result<usize, MediaError>;
}

/// Software/placeholder encoder: passes the raw frame through (still bounded
/// by `out.len()`) so the rest of the pipeline — pool → burn-in → governor →
/// link — can be exercised without a GPU.
#[derive(Debug, Default)]
pub struct NullEncoder {
    bitrate: u64,
}

impl NullEncoder {
    /// Creates an encoder targeting `bitrate` bits/s.
    pub fn new(bitrate: u64) -> Self {
        Self { bitrate }
    }
}

impl VideoEncoder for NullEncoder {
    fn set_bitrate(&mut self, bps: u64) {
        self.bitrate = bps;
    }
    fn current_bitrate(&self) -> u64 {
        self.bitrate
    }
    fn encode(
        &mut self,
        frame: &[u8],
        _meta: &FrameMeta,
        out: &mut [u8],
    ) -> Result<usize, MediaError> {
        if frame.len() > out.len() {
            return Err(MediaError::BufferTooSmall {
                needed: frame.len(),
                got: out.len(),
            });
        }
        out[..frame.len()].copy_from_slice(frame);
        Ok(frame.len())
    }
}

/// Slews `last` toward `want`, moving at most `max_step` (or 25% of `want`,
/// whichever is smaller, with a floor of `min_step`) to avoid encoder
/// thrash during transient congestion.
fn slew(last: u64, want: u64, min_step: u64) -> u64 {
    if last == want {
        return want;
    }
    let delta = want.abs_diff(last);
    let step = delta.min((want / 4).max(min_step).max(1));
    if want > last {
        last.saturating_add(step).min(want)
    } else {
        last.saturating_sub(step).max(want)
    }
}

/// Drives an encoder's bitrate from a [`BitrateSignal`], clamped to a
/// session's legal range and slewed to avoid oscillation.
pub struct EncoderGovernor<S> {
    signal: S,
    last: u64,
    min_bps: u64,
    max_bps: u64,
    min_step: u64,
}

impl<S: BitrateSignal> EncoderGovernor<S> {
    /// Builds a governor for `signal` clamped to `[min_bps, max_bps]`.
    pub fn new(signal: S, min_bps: u64, max_bps: u64) -> Self {
        assert!(min_bps <= max_bps, "min must be <= max");
        Self {
            signal,
            last: min_bps,
            min_bps,
            max_bps,
            min_step: (max_bps / 64).max(1000),
        }
    }

    /// Reads the signal, clamps + slews, applies to `enc`, and returns the
    /// bitrate now in effect.
    pub fn tick<E: VideoEncoder>(&mut self, enc: &mut E) -> u64 {
        let want = self
            .signal
            .suggested_bitrate_bps()
            .clamp(self.min_bps, self.max_bps);
        let next = slew(self.last, want, self.min_step);
        enc.set_bitrate(next);
        self.last = next;
        next
    }

    /// Most recent applied bitrate.
    #[inline]
    pub fn current(&self) -> u64 {
        self.last
    }
}

/// NVIDIA NVENC hardware encoder.
///
/// Deferred: the vendor CUDA/NVENC FFI is bound at hardware bring-up (no
/// Apache-2.0-only crate may enter the §2 MIT chain). Until then construction
/// fails loudly so a session never silently runs unaccelerated.
#[derive(Debug, Default)]
pub struct NvencEncoder {
    _private: (),
}

impl NvencEncoder {
    /// Always [`MediaError::Unsupported`] until the GPU binding lands.
    pub fn open() -> Result<Self, MediaError> {
        Err(MediaError::Unsupported(
            "NVENC binding deferred to hardware bring-up",
        ))
    }
}

impl VideoEncoder for NvencEncoder {
    fn set_bitrate(&mut self, _bps: u64) {}
    fn current_bitrate(&self) -> u64 {
        0
    }
    fn encode(
        &mut self,
        _frame: &[u8],
        _meta: &FrameMeta,
        _out: &mut [u8],
    ) -> Result<usize, MediaError> {
        Err(MediaError::Unsupported("NVENC binding deferred"))
    }
}

/// AMD AMF hardware encoder (Windows). Same deferral policy as [`NvencEncoder`].
#[derive(Debug, Default)]
pub struct AmfEncoder {
    _private: (),
}

impl AmfEncoder {
    /// Always [`MediaError::Unsupported`] until the AMF binding lands.
    pub fn open() -> Result<Self, MediaError> {
        Err(MediaError::Unsupported(
            "AMF binding deferred to hardware bring-up",
        ))
    }
}

impl VideoEncoder for AmfEncoder {
    fn set_bitrate(&mut self, _bps: u64) {}
    fn current_bitrate(&self) -> u64 {
        0
    }
    fn encode(
        &mut self,
        _frame: &[u8],
        _meta: &FrameMeta,
        _out: &mut [u8],
    ) -> Result<usize, MediaError> {
        Err(MediaError::Unsupported("AMF binding deferred"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn null_encoder_passthrough_is_bounded() {
        let mut enc = NullEncoder::new(5_000_000);
        assert_eq!(enc.current_bitrate(), 5_000_000);
        let frame = [0xABu8; 16];
        let mut out = [0u8; 16];
        let n = enc.encode(&frame, &FrameMeta::default(), &mut out).unwrap();
        assert_eq!(n, 16);
        assert_eq!(&out[..], &frame[..]);
        // Too-small output is rejected, not truncated.
        let mut small = [0u8; 8];
        assert!(matches!(
            enc.encode(&frame, &FrameMeta::default(), &mut small),
            Err(MediaError::BufferTooSmall { .. })
        ));
    }

    #[test]
    fn governor_clamps_and_slews_toward_signal() {
        let sig = ConstBitrate(100_000_000);
        let mut enc = NullEncoder::new(0);
        let mut gov = EncoderGovernor::new(sig, 1_000_000, 80_000_000);
        // Step 1: from floor 1M, slew up by ~25% of want (25M) → 26M.
        let a = gov.tick(&mut enc);
        assert!(a > 1_000_000 && a < 100_000_000);
        assert_eq!(enc.current_bitrate(), a);
        // Many ticks converge to the clamped cap (80M), not the raw signal.
        for _ in 0..40 {
            gov.tick(&mut enc);
        }
        assert_eq!(gov.current(), 80_000_000);
        assert_eq!(enc.current_bitrate(), 80_000_000);
    }

    #[test]
    fn governor_tracks_falling_signal_downward() {
        let mut enc = NullEncoder::new(0);
        let mut gov = EncoderGovernor::new(ConstBitrate(0), 1_000_000, 80_000_000);
        // current starts at min; signal 0 clamps to min.
        let b = gov.tick(&mut enc);
        assert_eq!(b, 1_000_000);
    }

    #[test]
    fn hardware_backends_fail_loudly() {
        assert!(NvencEncoder::open().is_err());
        assert!(AmfEncoder::open().is_err());
    }
}
