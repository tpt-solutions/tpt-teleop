//! Custom UDP multiplexer (control + telemetry + WebRTC ICE on one port),
//! event-loop networking, QUIC fallback, and swarm mesh discovery.
//!
//! Scaffold — implementation lands with Phase 7 of the roadmap.

/// Crate version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    #[test]
    fn skeleton_compiles() {
        assert!(!super::VERSION.is_empty());
    }
}
