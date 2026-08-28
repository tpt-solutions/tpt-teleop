//! Example 1 — full pipeline, single command.
//!
//! Builds the `PipelineHarness`, pushes one normalized command through the entire
//! zero-copy path (Ingest → Normalize → Route → Safety → Serialize → Transmit →
//! Receive), and prints the command as received on the wire.

use tpt_t_core::Mode;
use tpt_t_integration::PipelineHarness;
use tpt_t_safety::axis;

fn main() {
    let mut h = PipelineHarness::build().expect("harness");
    let now = 1_000_000u64;
    let cmd = tpt_t_core::ser::ControlCommand::zeroed(Mode::FullTeleop);
    let rx = h
        .step_direct(cmd, now)
        .expect("step")
        .expect("command received");

    println!("received command over the wire:");
    println!("  mode      = {:?}", rx.mode().unwrap_or(Mode::FullTeleop));
    println!("  roll      = {:.4}", rx.axes[axis::ROLL]);
    println!("  throttle  = {:.4}", rx.axes[axis::THROTTLE]);
    println!("  routed    = {}", h.routed());
}
