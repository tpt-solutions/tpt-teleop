//! Wait-free SPSC ring buffers, zero-copy pointer passing, and byte-to-struct
//! casting utilities for tpt-teleop.
//!
//! # Design
//!
//! * [`SpscRing`] — heap-backed, wait-free, bounded SPSC queue. The producer
//!   owns the tail cursor, the consumer owns the head cursor; neither op ever
//!   blocks, spins, or allocates. Capacity is rounded up to a power of two so
//!   indexing is a single AND.
//! * [`shared::SharedSpsc`] — the same algorithm attached to caller-provided
//!   memory (shared memory segments or mmapped regions), enabling true
//!   cross-process IPC with identical semantics.
//! * [`ptr`] — pointer-passing rings: producers hand pooled buffers to
//!   consumers without copying payload bytes.
//! * [`cast`] — zero-copy byte slice ↔ struct casting for `#[repr(C)]`
//!   plain-old-data types (the "Normalize" step of spec §6).
//! * [`layout`] — deterministic shared-memory byte layout math used by both
//!   the heap allocator path and future shm/mmap backends.
//!
//! # Safety contract
//!
//! All types here expose `&self` mutation through atomics only. `Send`/`Sync`
//! impls are sound because each slot is exclusively owned by exactly one side
//! at any time: indices in `[head, tail)` are readable by the consumer only;
//! `[tail, head + capacity)` are writable by the producer only. Cursors are
//! monotonically increasing `usize` values; wrap-around arithmetic relies on
//! two's-complement wrapping subtraction, which stays correct across the
//! entire `usize` range as long as capacity is a power of two.

#![warn(missing_debug_implementations)]

pub mod cache_line;
pub mod cast;
pub mod layout;
pub mod ptr;
pub mod shared;
pub mod spsc;

pub use cache_line::CachePadded;
pub use ptr::{PointerRing, PointerRingExt, Ptr};
pub use shared::{SharedHeader, SharedSpsc};
pub use spsc::SpscRing;

/// Crate version (from Cargo metadata).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
