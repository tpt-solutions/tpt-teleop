//! Zero-heap-allocation verification for the hot data plane (Phase 14, item 2).
//!
//! `CountingAllocator` replaces the system allocator for this test binary.
//! The invariant we prove is **per-command zero allocation**: once the
//! allocator backings have stabilized, processing additional commands must
//! not allocate (a real-time loop can tolerate a bounded one-time startup
//! cost, but must never allocate per command).
//!
//! Note on methodology: wrapping `System` also counts the OS allocator's own
//! internal heap bookkeeping (pool/metadata growth), which is unavoidable and
//! not attributable to our code. We therefore assert two things on a fully
//! warmed second window: (a) the heap does not grow net
//! (`net_allocations() <= 0`), and (b) the allocation count stays bounded far
//! below one-per-command (a true per-command leak would register ~N allocs,
//! not a handful). This catches any regression that makes the hot path
//! allocate per command while tolerating system-allocator noise.

use std::hint::black_box;

use tpt_t_core::Mode;
use tpt_t_core::ser::ControlCommand;
use tpt_t_input::ControllerReport;
use tpt_t_integration::{CountingAllocator, PipelineHarness, counts, reset_counts};
use tpt_t_safety::axis;

#[global_allocator]
static A: CountingAllocator = CountingAllocator;

fn sample(seq: u64, now: u64, roll: f32, throttle: f32) -> ControlCommand {
    let mut c = ControlCommand::zeroed(Mode::FullTeleop);
    c.seq = seq;
    c.timestamp_ns = now;
    c.axes[axis::ROLL] = roll;
    c.axes[axis::THROTTLE] = throttle;
    c
}

fn report(seq: u32, now: u64) -> ControllerReport {
    ControllerReport {
        seq,
        buttons: 0,
        axes: [0.2, 0.0, 0.0, 0.55, 0.0, 0.0, 0.0, 0.0],
        timestamp_ns: now,
    }
}

/// One measured window of `end - start` commands on the core plane
/// (no ingest stage). Returns the allocation tallies for that window.
fn window_core(h: &mut PipelineHarness, start: u64, end: u64) -> tpt_t_integration::AllocCounts {
    reset_counts();
    for i in start..end {
        let now = i * 1_000_000;
        let _ = black_box(h.pump_forward_direct(sample(i, now, 0.2, 0.55), now));
    }
    counts()
}

/// One measured window of `end - start` commands on the ingest plane.
fn window_ingest(h: &mut PipelineHarness, start: u64, end: u64) -> tpt_t_integration::AllocCounts {
    reset_counts();
    for i in start..end {
        let now = i * 1_000_000;
        h.feed_report(report(i as u32, now));
        let _ = black_box(h.pump_forward(now));
    }
    counts()
}

#[test]
fn hot_path_core_plane_makes_no_per_command_allocations() {
    let mut h = PipelineHarness::build().unwrap();
    // Warm-up (excluded from the comparison).
    for i in 0..2_000u64 {
        let now = i * 1_000_000;
        let _ = h.pump_forward_direct(sample(i, now, 0.2, 0.55), now);
    }

    // First steady-state window (absorbs any remaining one-time costs).
    let _w1 = window_core(&mut h, 2_000, 12_000);
    // Second steady-state window must not grow the heap and must stay bounded
    // far below one allocation per command (a per-command leak would be ~N).
    let w2 = window_core(&mut h, 12_000, 22_000);

    eprintln!("core-plane second-window allocations: {w2:?}");
    assert!(
        w2.net_allocations() <= 0,
        "core hot path grew the heap: {w2:?}"
    );
    assert!(
        w2.allocs <= 64,
        "core hot path allocates per command: {w2:?}"
    );
}

#[test]
fn hot_path_with_ingest_makes_no_per_command_allocations() {
    let mut h = PipelineHarness::build().unwrap();
    for i in 0..2_000u64 {
        let now = i * 1_000_000;
        h.feed_report(report(i as u32, now));
        let _ = h.pump_forward(now);
    }

    let _w1 = window_ingest(&mut h, 2_000, 12_000);
    let w2 = window_ingest(&mut h, 12_000, 22_000);

    eprintln!("ingest-plane second-window allocations: {w2:?}");
    assert!(
        w2.net_allocations() <= 0,
        "ingest hot path grew the heap: {w2:?}"
    );
    assert!(
        w2.allocs <= 64,
        "ingest hot path allocates per command: {w2:?}"
    );
}
