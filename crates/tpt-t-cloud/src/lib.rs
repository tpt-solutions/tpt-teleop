//! tpt-teleop cloud + multi-tenancy (Phase 10, spec §5.6).
//!
//! Fleet management server, WebRTC SFU, session recording, and an MCP server
//! for AI fleet dispatch — built with **no hyper / axum / tokio** and no
//! Apache-only transport crates.
//!
//! > **Dependency-policy note (consistent with Phase 7).** The roadmap names
//! > `quinn` (HTTP/3) and `webrtc-rs` (SFU) for this phase. Both pull `tokio`
//! > plus Apache-2.0-only branches and are banned by the workspace §2 MIT
//! > chain (see `deny.toml`). This crate therefore meets the same API
//! > contract with in-house components:
//! >
//! > * a custom HTTP/1.1 server driven by the Phase 2 platform event loop
//! >   ([`server`]) — the QUIC/HTTP3 slot is filled by our own reliable
//! >   transport behind the same trait surface;
//! > * an in-house selective-repeat SFU media router over lock-free SPSC rings
//! >   ([`sfu`]) instead of `webrtc-rs`;
//! > * UDP command transport reusing the Phase 7 [`tpt_t_link`] multiplexer
//! >   ([`fleet::UdpTransport`]).
//! >
//! > A `quinn`/WebRTC transport may be swapped in behind the same traits if
//! > dependency policy changes.

pub mod auth;
pub mod error;
pub mod fleet;
pub mod http;
pub mod json;
pub mod mcp;
pub mod recorder;
pub mod server;
pub mod sfu;

pub use auth::{authenticate_request, authorize_tool, extract_attestation};
pub use error::CloudError;
pub use fleet::{CapturingTransport, Fleet, NullTransport, SecureUdpTransport, UdpTransport, UnitState, UnitTransport};
pub use http::{Method, Request, Response};
pub use json::Json;
pub use mcp::McpServer;
pub use recorder::{FileRecorder, NullRecorder, Recorder, VecRecorder};
pub use server::{FleetServer, ServerLimits};
pub use sfu::{MediaFrame, SfuFanout};

/// Crate version (from Cargo metadata).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// UNIX-epoch nanoseconds (wall clock) used for command timestamps and FPV.
pub(crate) fn now_unix_ns() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    #[test]
    fn skeleton_compiles() {
        assert!(!super::VERSION.is_empty());
    }
}
