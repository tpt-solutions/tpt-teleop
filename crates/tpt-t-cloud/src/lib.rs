//! Minimal HTTP/3 fleet server and WebRTC SFU built directly on quinn +
//! socket2. No hyper, no axum, no tokio.
//!
//! Scaffold — implementation lands with Phase 10 of the roadmap.

/// Crate version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    #[test]
    fn skeleton_compiles() {
        assert!(!super::VERSION.is_empty());
    }
}
