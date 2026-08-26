//! Flight Data Recorder (FDR) writing via direct I/O (O_DIRECT /
//! FILE_FLAG_NO_BUFFERING / F_NOCACHE) plus AI-training pipeline export
//! (rkyv wire buffers → NumPy `.npy`, the lingua franca for both PyTorch and
//! JAX — see `export`).
//!
//! # Design (spec §5.8)
//!
//! Logging must **never block the control loop**. The hot path therefore never
//! touches a disk: it publishes fixed-size [`record::FdrEntry`] frames through
//! a wait-free SPSC ring ([`tpt_t_ring::SpscRing`]) owned by the analytics
//! crate. A dedicated storage thread (role "Storage/FDR" in the core-pinning
//! profile) drains the ring and writes the bytes straight to a
//! [`direct_io::DirectFile`], which uses the OS direct-I/O path so the page
//! cache is bypassed and the writer's `write` never stalls behind dirty-page
//! writeback. If the ring is full the producer's `try_*` call fails
//! immediately (record shed) — it never spins or blocks.
//!
//! The AI export path is entirely offline: a finished FDR file (or an in-memory
//! set of entries) is turned into feature/label tensors and serialized as
//! NumPy `.npy` arrays that PyTorch (`numpy.load` → `torch.from_numpy`) and
//! JAX (`numpy.load` → `jnp.asarray`) both consume natively.

#![allow(missing_debug_implementations)]

pub mod direct_io;
pub mod export;
pub mod npy;
pub mod record;

pub use direct_io::DirectFile;
pub use export::{AiDataset, Sample, extract_features};
pub use npy::{NpyView, f32_npy, f64_npy, read_npy, write_npy};
pub use record::{FdrEntry, FdrSink, FdrWriter, RecordError, RecordKind, Recorder, parse_entries};

/// Crate version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
