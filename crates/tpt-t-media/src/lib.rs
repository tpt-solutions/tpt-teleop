//! Zero-copy camera ingestion, slab frame pools, hardware-accelerated
//! encoding, and AR HUD telemetry burn-in.
//!
//! Scaffold — implementation lands with Phase 8 of the roadmap.

/// Crate version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    #[test]
    fn skeleton_compiles() {
        assert!(!super::VERSION.is_empty());
    }
}
