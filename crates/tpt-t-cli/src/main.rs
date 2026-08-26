//! tpt-teleop CLI: project scaffolding, cargo-deny MIT-chain config
//! generation, and CPU core-pinning profile setup (spec §13).
//!
//! ```text
//! tpt-t-cli scaffold <NAME> [--path <DIR>]
//! tpt-t-cli deny     [--out <FILE>]
//! tpt-t-cli profile  [--cores <N>] [--out <FILE>]
//! tpt-t-cli help
//! ```

mod deny;
mod profile;
mod scaffold;

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
    tpt-t-cli help                              Show this message\n",
        env!("CARGO_PKG_VERSION")
    );
}
