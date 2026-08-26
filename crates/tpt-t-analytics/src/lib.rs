//! Flight Data Recorder writing via direct I/O (O_DIRECT /
//! FILE_FLAG_NO_BUFFERING / F_NOCACHE) plus AI-training pipeline export.
//!
//! Scaffold — implementation lands with Phase 12 of the roadmap.

/// Crate version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    #[test]
    fn skeleton_compiles() {
        assert!(!super::VERSION.is_empty());
    }
}
