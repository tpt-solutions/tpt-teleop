//! tpt-teleop link layer (spec §5.2): everything between the safety loop's
//! output ring and the wire.
//!
//! * [`mux`] — custom UDP multiplexer: control, telemetry, media, WebRTC ICE,
//!   and mesh traffic share **one** port (default UDP 443) behind an 8-byte
//!   [`tpt_t_core::ser::WireFrame`] header plus a 4-byte channel/flags word.
//!   rkyv serialization writes straight into the caller's datagram buffer —
//!   no intermediate allocation.
//! * [`service`] — drives [`mux::UdpMux`] from the Phase 2 platform event
//!   loop (epoll / kqueue / IOCP), one pinned network thread, no async
//!   runtime anywhere.
//! * [`uring`] *(Linux)* — genuine io_uring transmit path; datagrams are
//!   encoded directly into submission-owned slots so the kernel reads what
//!   the serializer wrote.
//! * [`reliable`] — selective-repeat ARQ giving the control channel ordered,
//!   loss-recovering delivery when raw UDP is not enough. This is the QUIC
//!   fallback slot: quinn itself cannot ship under the §2 MIT chain (its
//!   tree pulls tokio plus Apache-2.0-only rustls/aws-lc-rs branches), so
//!   the same API contract is met by this in-house protocol. Swap-in remains
//!   possible if dependency policy changes.
//! * [`mesh`] — swarm neighbor discovery: periodic signed-sequence beacons
//!   and a fixed-size neighbor table with TTL expiry.
//! * [`backpressure`] — lock-free congestion/token-bucket signal computed on
//!   the network thread and consumed by the media encoder (Phase 8 wiring).
//! * [`crc`] — dependency-free CRC32 for frame trailers and command tags.

pub mod backpressure;
pub mod crc;
pub mod mesh;
pub mod mux;
#[cfg(target_os = "linux")]
pub mod uring;
pub mod reliable;
pub mod service;

/// Crate version (from Cargo metadata).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");