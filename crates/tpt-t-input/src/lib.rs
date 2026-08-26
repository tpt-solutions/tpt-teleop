//! Raw HID controller polling, OpenXR hand tracking, haptics, and shared
//! co-pilot control state.
//!
//! Scaffold — implementation lands with Phase 6 of the roadmap.

/// Crate version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    #[test]
    fn skeleton_compiles() {
        assert!(!super::VERSION.is_empty());
    }
}
