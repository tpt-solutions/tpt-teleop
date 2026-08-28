//! tpt-teleop CLI: project scaffolding, cargo-deny MIT-chain config
//! generation, CPU core-pinning profile setup (spec §13), FDR replay, an MCP
//! fleet-dispatch console (Phase 17), a toolchain/environment `doctor`
//! (Phase 16), and a live simulator readout driving the Phase 4 hardware+
//! safety pipeline end-to-end (Phase 17).
//!
//! ```text
//! tpt-t-cli scaffold <NAME> [--path <DIR>]
//! tpt-t-cli deny     [--out <FILE>]
//! tpt-t-cli profile  [--cores <N>] [--out <FILE>]
//! tpt-t-cli replay   <FILE> [--speed <N>] [--kind <control|imu|gps|telemetry>] [--limit <N>]
//! tpt-t-cli console  [--host <ADDR:PORT>] [--attestation <FILE>]
//! tpt-t-cli doctor
//! tpt-t-cli sim      [--ticks <N>] [--rate <HZ>] [--throttle <0..1>] [--roll <RAD>]
//! tpt-t-cli help
//! ```

mod console;
mod deny;
mod doctor;
mod profile;
mod replay;
mod scaffold;
mod sim;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = run(&args);
    std::process::exit(code);
}

/// Dispatches to a subcommand. Returns the process exit code.
fn run(args: &[String]) -> i32 {
    match args.first().map(String::as_str) {
        Some("scaffold") => scaffold::run(&args[1..]),
        Some("deny") => deny::run(&args[1..]),
        Some("profile") => profile::run(&args[1..]),
        Some("replay") => replay::run(&args[1..]),
        Some("console") => console::run(&args[1..]),
        Some("doctor") => doctor::run(&args[1..]),
        Some("sim") => sim::run(&args[1..]),
        Some("help") | Some("-h") | Some("--help") | None => {
            print_usage();
            0
        }
        Some(other) => {
            eprintln!("error: unknown subcommand {other:?}");
            print_usage();
            1
        }
    }
}

fn print_usage() {
    println!(
        "tpt-t-cli v{} — tpt-teleop developer tooling\n\
\n\
USAGE:\n\
    tpt-t-cli scaffold <NAME> [--path <DIR>]   Scaffold a new robot crate\n\
    tpt-t-cli deny     [--out <FILE>]          Emit cargo-deny MIT-chain config\n\
    tpt-t-cli profile  [--cores <N>] [--out <FILE>]  Emit a CPU core-pinning profile\n\
    tpt-t-cli replay   <FILE> [--speed <N>] [--kind <K>] [--limit <N>]  Replay an FDR file\n\
    tpt-t-cli console  [--host <ADDR:PORT>] [--attestation <FILE>]  MCP fleet-dispatch console\n\
    tpt-t-cli doctor                              Check toolchain/environment\n\
    tpt-t-cli sim      [--ticks <N>] [--rate <HZ>] [--throttle <F>] [--roll <R>]  Live simulator\n\
    tpt-t-cli help                              Show this message\n",
        env!("CARGO_PKG_VERSION")
    );
}
