//! `tpt-t-cli replay <FILE>` — offline FDR replay/visualization (spec §5.8,
//! Phase 17).
//!
//! Reads a flight-data-recorder file written by
//! `tpt_t_analytics::record::Recorder` and prints a decoded, human-readable
//! trace: a per-kind summary, then one line per frame with typed field
//! values for `ControlCommand`, `ImuSample`, `GpsSample`, and
//! `TelemetryPacket`. No new external dependencies — this is a thin terminal
//! front-end over `tpt_t_analytics::{parse_entries, record::from_bytes_aligned}`.
//!
//! ```text
//! tpt-t-cli replay <FILE> [--speed <N>] [--kind <control|imu|gps|telemetry>] [--limit <N>]
//! ```

use std::thread;
use std::time::Duration;

use tpt_t_analytics::record::from_bytes_aligned;
use tpt_t_analytics::{FdrEntry, RecordKind, parse_entries};
use tpt_t_core::mode::Mode;
use tpt_t_core::ser::{ControlCommand, GpsSample, ImuSample, TelemetryKind, TelemetryPacket};

pub fn run(args: &[String]) -> i32 {
    let mut path: Option<String> = None;
    let mut speed: Option<f64> = None;
    let mut kind_filter: Option<RecordKind> = None;
    let mut limit: Option<usize> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--speed" => {
                i += 1;
                match args.get(i).and_then(|s| s.parse::<f64>().ok()) {
                    Some(v) if v > 0.0 => speed = Some(v),
                    _ => {
                        eprintln!("error: --speed requires a positive number");
                        return 1;
                    }
                }
            }
            "--kind" => {
                i += 1;
                match args.get(i).map(String::as_str).and_then(parse_kind) {
                    Some(k) => kind_filter = Some(k),
                    None => {
                        eprintln!("error: --kind must be one of control|imu|gps|telemetry");
                        return 1;
                    }
                }
            }
            "--limit" => {
                i += 1;
                match args.get(i).and_then(|s| s.parse::<usize>().ok()) {
                    Some(v) => limit = Some(v),
                    None => {
                        eprintln!("error: --limit requires a number");
                        return 1;
                    }
                }
            }
            other if path.is_none() && !other.starts_with("--") => path = Some(other.to_string()),
            other => {
                eprintln!("error: unrecognized argument {other:?}");
                return 1;
            }
        }
        i += 1;
    }

    let path = match path {
        Some(p) => p,
        None => {
            eprintln!(
                "usage: tpt-t-cli replay <FILE> [--speed <N>] [--kind <control|imu|gps|telemetry>] [--limit <N>]"
            );
            return 1;
        }
    };

    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("error: cannot read {path:?}: {e}");
            return 1;
        }
    };

    let entries = parse_entries(&bytes);
    if entries.is_empty() {
        println!("{path}: no frames");
        return 0;
    }

    print_summary(&path, &entries);

    let t0 = entries[0].timestamp_ns;
    let mut prev_ts: Option<u64> = None;
    let mut printed = 0usize;
    for e in &entries {
        if let Some(k) = kind_filter {
            if e.kind() != Some(k) {
                continue;
            }
        }
        if let Some(max) = limit {
            if printed >= max {
                break;
            }
        }
        if let Some(mult) = speed {
            if let Some(prev) = prev_ts {
                let delta_ns = e.timestamp_ns.saturating_sub(prev);
                if delta_ns > 0 {
                    thread::sleep(Duration::from_nanos((delta_ns as f64 / mult) as u64));
                }
            }
        }
        println!("{}", decode_line(printed, e, t0));
        prev_ts = Some(e.timestamp_ns);
        printed += 1;
    }
    println!("-- {printed} frame(s) shown --");
    0
}

fn parse_kind(s: &str) -> Option<RecordKind> {
    match s {
        "control" => Some(RecordKind::Control),
        "imu" => Some(RecordKind::Imu),
        "gps" => Some(RecordKind::Gps),
        "telemetry" => Some(RecordKind::Telemetry),
        _ => None,
    }
}

fn print_summary(path: &str, entries: &[FdrEntry]) {
    let mut counts = [0usize; 4];
    for e in entries {
        match e.kind() {
            Some(RecordKind::Control) => counts[0] += 1,
            Some(RecordKind::Imu) => counts[1] += 1,
            Some(RecordKind::Gps) => counts[2] += 1,
            Some(RecordKind::Telemetry) => counts[3] += 1,
            _ => {}
        }
    }
    let span_ns = entries
        .last()
        .expect("checked non-empty")
        .timestamp_ns
        .saturating_sub(entries[0].timestamp_ns);
    println!("== {path} ==");
    println!(
        "frames={} control={} imu={} gps={} telemetry={} span={:.3}s",
        entries.len(),
        counts[0],
        counts[1],
        counts[2],
        counts[3],
        span_ns as f64 / 1e9
    );
    println!("---");
}

fn decode_line(idx: usize, e: &FdrEntry, t0: u64) -> String {
    let rel_ms = e.timestamp_ns.saturating_sub(t0) as f64 / 1e6;
    match e.kind() {
        Some(RecordKind::Control) => match from_bytes_aligned::<ControlCommand>(e.payload()) {
            Some(c) => format!(
                "[{idx:>6}] t=+{rel_ms:>10.3}ms CONTROL   seq={:<6} mode={:<12} ai={:<5} axes={:?}",
                c.seq,
                c.mode().map(Mode::name).unwrap_or("?"),
                c.is_ai_origin(),
                c.axes
            ),
            None => format!("[{idx:>6}] t=+{rel_ms:>10.3}ms CONTROL   <malformed>"),
        },
        Some(RecordKind::Imu) => match from_bytes_aligned::<ImuSample>(e.payload()) {
            Some(s) => format!(
                "[{idx:>6}] t=+{rel_ms:>10.3}ms IMU       seq={:<6} gyro_rps={:?} accel_g={:?}",
                s.seq, s.gyro_rps, s.accel_g
            ),
            None => format!("[{idx:>6}] t=+{rel_ms:>10.3}ms IMU       <malformed>"),
        },
        Some(RecordKind::Gps) => match from_bytes_aligned::<GpsSample>(e.payload()) {
            Some(s) => format!(
                "[{idx:>6}] t=+{rel_ms:>10.3}ms GPS       seq={:<6} lat={:.6} lon={:.6} alt={:.1}m speed={:.1}m/s sats={} fix={}",
                s.seq,
                s.lat_deg,
                s.lon_deg,
                s.alt_m,
                s.speed_mps,
                s.sats,
                s.fix_ok != 0
            ),
            None => format!("[{idx:>6}] t=+{rel_ms:>10.3}ms GPS       <malformed>"),
        },
        Some(RecordKind::Telemetry) => match from_bytes_aligned::<TelemetryPacket>(e.payload()) {
            Some(p) => format!(
                "[{idx:>6}] t=+{rel_ms:>10.3}ms TELEMETRY seq={:<6} kind={:<12} values={:?}",
                p.seq,
                TelemetryKind::from_u16(p.kind)
                    .map(|k| format!("{k:?}"))
                    .unwrap_or_else(|| "?".into()),
                p.values
            ),
            None => format!("[{idx:>6}] t=+{rel_ms:>10.3}ms TELEMETRY <malformed>"),
        },
        Some(RecordKind::End) | None => format!("[{idx:>6}] t=+{rel_ms:>10.3}ms <end/unknown>"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tpt_t_analytics::record::Recorder;

    fn write_sample_fdr(path: &std::path::Path) {
        let rec = Recorder::open(path, 64).unwrap();
        let cmd = ControlCommand::zeroed(Mode::FullTeleop);
        rec.sink().try_control(&cmd).unwrap();
        let imu = ImuSample::zeroed(1, 100);
        rec.sink().try_imu(&imu).unwrap();
        let gps = GpsSample::zeroed(2, 200);
        rec.sink().try_gps(&gps).unwrap();
        let tel = TelemetryPacket::zeroed(TelemetryKind::Battery, 3, 300);
        rec.sink().try_telemetry(&tel).unwrap();
        rec.stop_and_join().unwrap();
    }

    #[test]
    fn replay_decodes_every_frame_kind() {
        let dir = std::env::temp_dir().join("tpt_cli_replay_test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join(format!("fdr_{}.bin", std::process::id()));
        write_sample_fdr(&path);

        let bytes = std::fs::read(&path).unwrap();
        let entries = parse_entries(&bytes);
        assert_eq!(entries.len(), 4);

        let t0 = entries[0].timestamp_ns;
        for (i, e) in entries.iter().enumerate() {
            let line = decode_line(i, e, t0);
            assert!(!line.contains("<malformed>"), "line: {line}");
        }

        let code = run(&[path.to_string_lossy().to_string()]);
        assert_eq!(code, 0);

        let code = run(&[
            path.to_string_lossy().to_string(),
            "--kind".into(),
            "imu".into(),
            "--limit".into(),
            "1".into(),
        ]);
        assert_eq!(code, 0);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn missing_file_argument_is_an_error() {
        assert_eq!(run(&[]), 1);
    }

    #[test]
    fn bad_kind_flag_is_an_error() {
        assert_eq!(run(&["f.bin".into(), "--kind".into(), "bogus".into()]), 1);
    }
}
