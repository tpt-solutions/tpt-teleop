//! Integration crate for Phase 14: a single binary that proves the whole
//! zero-copy data plane works end to end, that the hot path makes no heap
//! allocations and takes no locks, and that the cross-subsystem latency
//! budget holds.
//!
//! The [`pipeline::PipelineHarness`] wires the stages exactly in the order the
//! spec §6 diagram prescribes:
//!
//! ```text
//! Ingest ──▶ Normalize ──▶ Route ──▶ Safety ──▶ Serialize ──▶ Transmit
//! ```
//!
//! * **Ingest** — a [`tpt_t_input::InputStage`] polling a scripted HID source.
//! * **Normalize** — the [`tpt_t_input::ControllerMap`] mapping the report
//!   onto a [`tpt_t_core::ser::ControlCommand`].
//! * **Route** — a lock-free [`tpt_t_core::bus::MessageBus`] fan-out that
//!   delivers the normalized command to the safety loop (and a logger
//!   subscriber), proving the routing stage.
//! * **Safety** — the [`tpt_t_safety::SafetyLoop`] deterministic intercept.
//! * **Serialize** — [`tpt_t_link::UdpMux::write_control_frame`] (rkyv into a
//!   reused aligned scratch buffer).
//! * **Transmit** — [`tpt_t_link::UdpMux::send_framed`] over a real loopback
//!   UDP socket, then demultiplexed back on the receive side.
//!
//! [`alloc`] provides a `GlobalAlloc` counting wrapper used by the
//! zero-allocation hot-path verification test.

pub mod alloc;
pub mod pipeline;

pub use alloc::{AllocCounts, CountingAllocator, counts, reset_counts};
pub use pipeline::PipelineHarness;

/// Crate version (from Cargo metadata).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
