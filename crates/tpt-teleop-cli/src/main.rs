//! tpt-teleop CLI: project scaffolding, cargo-deny MIT-chain config
//! generation, and CPU core-pinning profile setup.
//!
//! Scaffold binary; subcommands arrive with Phase 13 of the roadmap.

fn main() {
    println!(
        "tpt-teleop-cli v{} — scaffolding, deny-config, and core-pinning tooling (scaffold)",
        env!("CARGO_PKG_VERSION")
    );
}
