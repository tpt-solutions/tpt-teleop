//! Phase 8 end-to-end pipeline test (acceptance):
//! capture → slab pool → telemetry burn-in → encode → bitrate governor
//! driven by the Phase 7 network backpressure signal.
//!
//! Runs entirely on the simulator sources so it exercises every software
//! path without GPU/OS camera dependencies.

use tpt_t_core::ser::{TelemetryKind, TelemetryPacket};
use tpt_t_link::backpressure::Backpressure;
use tpt_t_media::burnin::burn_in_telemetry;
use tpt_t_media::capture::{CaptureBackend, TestPatternCapture};
use tpt_t_media::encoder::{ConstBitrate, EncoderGovernor, NullEncoder, VideoEncoder};
use tpt_t_media::pool::{FrameMeta, FramePool, PixFmt};

#[test]
fn pipeline_capture_to_encode_with_burnin() {
    let mut cam = TestPatternCapture::new(64, 32, PixFmt::Rgb888);
    let mut pool = FramePool::new(PixFmt::Rgb888.min_buffer_len(64, 32), 4);
    let mut enc = NullEncoder::new(0);
    // Output storage is reused across frames (plain Vec allocated once).
    let mut out = vec![0u8; PixFmt::Rgb888.min_buffer_len(64, 32)];

    let pkt = TelemetryPacket {
        values: [87.0, 12.5, 3.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        ..TelemetryPacket::zeroed(TelemetryKind::Battery, 1, 2)
    };

    for i in 0..3 {
        let mut frame = pool.alloc().expect("pool has a free block");
        let mut meta = FrameMeta::default();
        cam.grab(&mut frame, &mut meta).unwrap();
        assert_eq!(meta.seq as usize, i + 1);

        // Burn the telemetry HUD into the frame before encode.
        let before = frame[0];
        burn_in_telemetry(&mut frame, &meta, &pkt, 255);
        // Burn-in must have changed pixels somewhere in the top strip.
        assert!(frame.iter().any(|&b| b != before) || before != 0);

        let n = enc
            .encode(&frame, &meta, &mut out)
            .expect("encode fits output");
        assert_eq!(n, frame.len(), "null encoder passes the frame through");
        drop(frame); // returns block to the pool immediately
    }
}

#[test]
fn governor_tracks_backpressure_suggested_bitrate() {
    // A backpressure signal driven into heavy congestion should drag the
    // encoder bitrate down toward the signal's reduced suggestion.
    let bp = Backpressure::default();
    bp.set_queue_depth(64); // → Congestion::Critical (25% of capacity)
    for _ in 0..5 {
        bp.note_send_blocked(1);
    }
    assert!(bp.suggested_bitrate_bps() < bp.capacity_bps());

    let mut enc = NullEncoder::new(0);
    let mut gov = EncoderGovernor::new(&bp, 1_000_000, 100_000_000);

    // The applied bitrate follows the (downward) signal, clamped/slewed.
    let a = gov.tick(&mut enc);
    assert!(
        a <= bp.suggested_bitrate_bps() + 1,
        "applied bitrate tracks the congested signal"
    );
    assert_eq!(enc.current_bitrate(), a);

    // Upward recovery under a healthy constant signal converges to target.
    let mut enc2 = NullEncoder::new(0);
    let mut gov2 = EncoderGovernor::new(ConstBitrate(100_000_000), 1_000_000, 100_000_000);
    for _ in 0..40 {
        gov2.tick(&mut enc2);
    }
    assert_eq!(
        gov2.current(),
        100_000_000,
        "climbs to target under healthy signal"
    );
}

#[test]
fn const_signal_holds_steady_bitrate() {
    let sig = ConstBitrate(42_000_000);
    let mut enc = NullEncoder::new(0);
    let mut gov = EncoderGovernor::new(sig, 1_000_000, 100_000_000);
    // Slew ramps from the floor to the target over several ticks.
    for _ in 0..40 {
        gov.tick(&mut enc);
    }
    assert_eq!(
        gov.current(),
        42_000_000,
        "converges to the constant signal"
    );
    // Once converged it holds steady.
    let b = gov.tick(&mut enc);
    assert_eq!(b, 42_000_000);
    assert_eq!(enc.current_bitrate(), 42_000_000);
}
