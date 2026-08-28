//! `tpt-t-cli doctor` — toolchain/environment sanity check (Phase 16).
//!
//! A zero-dependency preflight that confirms the host can build and run the
//! workspace: the Rust toolchain, the target platform, the CPU topology used by
//! thread-per-core pinning, and the presence of optional helpers (e.g.
//! `cargo-deny`). It prints a report and exits non-zero if any *critical* check
//! fails, so it can gate CI or a developer's first build.

use std::process::Command;

/// Exit code on success.
const OK: i32 = 0;
/// Exit code on failure.
const FAIL: i32 = 1;

/// One row of the doctor report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// Short check name.
    pub name: &'static str,
    /// `true` when the check passed.
    pub ok: bool,
    /// Whether a failure here blocks (critical) or merely warns.
    pub critical: bool,
    /// Human-readable detail line.
    pub detail: String,
}

impl Finding {
    fn pass(name: &'static str, detail: impl Into<String>) -> Self {
        Self {
            name,
            ok: true,
            critical: true,
            detail: detail.into(),
        }
    }

    fn warn(name: &'static str, detail: impl Into<String>) -> Self {
        Self {
            name,
            ok: false,
            critical: false,
            detail: detail.into(),
        }
    }

    fn fail(name: &'static str, detail: impl Into<String>) -> Self {
        Self {
            name,
            ok: false,
            critical: true,
            detail: detail.into(),
        }
    }
}

/// Runs the toolchain/`cargo` command, returning its stdout trimmed (empty on
/// any failure so the caller can treat missing tools as not-found).
fn tool_version(program: &str, args: &[&str]) -> String {
    Command::new(program)
        .args(args)
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                String::from_utf8(o.stdout).ok()
            } else {
                None
            }
        })
        .unwrap_or_default()
        .trim()
        .to_string()
}

/// Checks the workspace is reachable from the current directory by walking up
/// the tree looking for `crates/tpt-t-core/Cargo.toml` (works whether `doctor`
/// is run from the workspace root or a crate subdirectory).
fn workspace_ok() -> bool {
    let mut dir = match std::env::current_dir() {
        Ok(d) => d,
        Err(_) => return false,
    };
    loop {
        if dir.join("crates/tpt-t-core/Cargo.toml").exists() {
            return true;
        }
        if !dir.pop() {
            return false;
        }
    }
}

/// Collects every finding without printing (unit-testable).
pub fn collect() -> Vec<Finding> {
    let mut out = Vec::new();

    // Rust toolchain.
    let rustc = tool_version("rustc", &["--version"]);
    if rustc.is_empty() {
        out.push(Finding::fail(
            "rustc",
            "rustc not found on PATH — install Rust >= 1.85",
        ));
    } else {
        out.push(Finding::pass("rustc", rustc));
    }
    let cargo = tool_version("cargo", &["--version"]);
    if cargo.is_empty() {
        out.push(Finding::fail(
            "cargo",
            "cargo not found on PATH — install Rust >= 1.85",
        ));
    } else {
        out.push(Finding::pass("cargo", cargo));
    }

    // Platform.
    out.push(Finding::pass(
        "platform",
        format!("{} / {}", std::env::consts::OS, std::env::consts::ARCH),
    ));

    // CPU topology used for thread-per-core pinning.
    let cores = tpt_t_core::affinity::core_count();
    out.push(Finding::pass(
        "cpu-cores",
        format!("{cores} logical cores available for pinning"),
    ));

    // Optional: cargo-deny for the license audit.
    let deny = tool_version("cargo-deny", &["--version"]);
    if deny.is_empty() {
        out.push(Finding::warn(
            "cargo-deny",
            "not installed — `cargo deny check` license audit unavailable (optional)",
        ));
    } else {
        out.push(Finding::pass("cargo-deny", deny));
    }

    // Workspace presence.
    if workspace_ok() {
        out.push(Finding::pass("workspace", "crates/tpt-t-core located"));
    } else {
        out.push(Finding::fail(
            "workspace",
            "run doctor from the workspace root (crates/tpt-t-core not found)",
        ));
    }

    out
}

/// doctor subcommand entry point.
pub fn run(_args: &[String]) -> i32 {
    println!("tpt-teleop doctor — environment check\n");
    let findings = collect();
    let mut critical_fail = false;
    for f in &findings {
        let mark = if f.ok {
            "ok"
        } else if f.critical {
            "FAIL"
        } else {
            "warn"
        };
        println!("  [{mark:>4}] {:<12} {detail}", f.name, detail = f.detail);
        if !f.ok && f.critical {
            critical_fail = true;
        }
    }
    let warns = findings.iter().filter(|f| !f.ok && !f.critical).count();
    let fails = findings.iter().filter(|f| !f.ok && f.critical).count();
    println!(
        "\n{} check(s) passed, {} warning(s), {} failure(s)",
        findings.len() - warns - fails,
        warns,
        fails
    );
    if critical_fail {
        println!("doctor: environment is NOT ready — resolve the FAIL items above");
        FAIL
    } else {
        println!("doctor: environment looks ready");
        OK
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_without_panicking_and_reports_rustc() {
        let findings = collect();
        assert!(!findings.is_empty());
        // rustc/cargo/workspace are always checked; on a dev machine rustc and
        // cargo are present and this test runs from the workspace root.
        let rustc = findings.iter().find(|f| f.name == "rustc").unwrap();
        assert!(rustc.ok, "rustc should be present in the test environment");
        let ws = findings.iter().find(|f| f.name == "workspace").unwrap();
        assert!(ws.ok, "workspace should be found from the crate test cwd");
    }

    #[test]
    fn platform_and_cores_report_sensibly() {
        let findings = collect();
        let cores = findings.iter().find(|f| f.name == "cpu-cores").unwrap();
        assert!(cores.ok);
        assert!(cores.detail.parse::<usize>().is_err()); // detail is prose
        let plat = findings.iter().find(|f| f.name == "platform").unwrap();
        assert!(plat.detail.contains(std::env::consts::OS));
    }
}
