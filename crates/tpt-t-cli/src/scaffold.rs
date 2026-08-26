//! `tpt-t-cli scaffold` — generate a new robot crate pre-wired with the
//! `#[derive(tpt_t::Robot)]` macro (spec §13 — scaffolding tooling).
//!
//! Run it from a tpt-teleop workspace root so the generated `path =`
//! dependencies resolve against `crates/`.

use std::path::{Path, PathBuf};

/// Exit code on success.
const OK: i32 = 0;
/// Exit code on failure.
const FAIL: i32 = 1;

/// Scaffold subcommand entry point.
pub fn run(args: &[String]) -> i32 {
    let mut name: Option<String> = None;
    let mut path: Option<String> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--path" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("error: --path requires a value");
                    return FAIL;
                }
                path = Some(args[i].clone());
            }
            other if other.starts_with("--path=") => {
                path = Some(other["--path=".len()..].to_string());
            }
            other if name.is_none() && !other.starts_with("--") => {
                name = Some(other.to_string());
            }
            other => {
                eprintln!("error: unexpected argument {other:?}");
                return FAIL;
            }
        }
        i += 1;
    }

    let name = match name {
        Some(n) => n,
        None => {
            eprintln!("error: scaffold requires a <NAME>");
            return FAIL;
        }
    };

    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        eprintln!("error: NAME must be alphanumeric/underscore/dash");
        return FAIL;
    }

    let out_dir = match path {
        Some(p) => PathBuf::from(p),
        None => std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
    };

    match scaffold(&name, &out_dir) {
        Ok(project_dir) => {
            println!(
                "scaffolded robot {name:?} at {}\n\
                 \n\
                 next steps:\n\
                 \tcd {}\n\
                 \tcargo build\n\
                 \t# wire your Camera/Motor types and run with `cargo run --release`",
                project_dir.display(),
                project_dir.display()
            );
            OK
        }
        Err(e) => {
            eprintln!("error: scaffold failed: {e}");
            FAIL
        }
    }
}

/// Creates the project directory and writes `Cargo.toml`, `src/main.rs`, and
/// `core-profile.txt`. Path dependencies are resolved relative to the nearest
/// workspace root (detected by locating `crates/tpt-t-core`).
fn scaffold(name: &str, out_dir: &Path) -> Result<PathBuf, String> {
    let project_dir = out_dir.join(name);
    if project_dir.exists() {
        return Err(format!(
            "target directory {} already exists",
            project_dir.display()
        ));
    }

    let struct_name = capitalize(name);

    // Resolve dependency paths against the workspace root when discoverable.
    let (core, ring, macros) = match find_workspace_root() {
        Some(root) => (
            rel_path(&project_dir, &root.join("crates/tpt-t-core"))
                .unwrap_or_else(|| "../crates/tpt-t-core".into()),
            rel_path(&project_dir, &root.join("crates/tpt-t-ring"))
                .unwrap_or_else(|| "../crates/tpt-t-ring".into()),
            rel_path(&project_dir, &root.join("crates/tpt-t-macros"))
                .unwrap_or_else(|| "../crates/tpt-t-macros".into()),
        ),
        None => (
            "../crates/tpt-t-core".into(),
            "../crates/tpt-t-ring".into(),
            "../crates/tpt-t-macros".into(),
        ),
    };

    std::fs::create_dir_all(project_dir.join("src"))
        .map_err(|e| format!("cannot create {}: {e}", project_dir.display()))?;

    let cargo_toml = CARGO_TOML
        .replace("__NAME__", name)
        .replace("__STRUCT__", &struct_name)
        .replace("__CORE__", &core)
        .replace("__RING__", &ring)
        .replace("__MACROS__", &macros);
    write_file(&project_dir.join("Cargo.toml"), &cargo_toml)?;

    let main_rs = MAIN_RS
        .replace("__NAME__", name)
        .replace("__STRUCT__", &struct_name);
    write_file(&project_dir.join("src/main.rs"), &main_rs)?;

    write_file(
        &project_dir.join("core-profile.txt"),
        &crate::profile::default_profile_text(tpt_t_core::affinity::core_count().max(1)),
    )?;

    Ok(project_dir)
}

/// Writes `content` to `path`, mapping IO errors to strings.
fn write_file(path: &Path, content: &str) -> Result<(), String> {
    std::fs::write(path, content).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

/// Converts `name` to UpperCamelCase for a generated struct identifier
/// (e.g. `demo_bot` → `DemoBot`, `my-bot` → `MyBot`).
fn capitalize(s: &str) -> String {
    let mut out = String::new();
    let mut upper = true;
    for c in s.chars() {
        if c == '_' || c == '-' {
            upper = true;
            continue;
        }
        if upper {
            out.extend(c.to_uppercase());
            upper = false;
        } else {
            out.push(c);
        }
    }
    if out.is_empty() {
        out.push_str("Bot");
    }
    out
}

/// Walks up from the current directory looking for a workspace that contains
/// `crates/tpt-t-core`.
fn find_workspace_root() -> Option<PathBuf> {
    let mut dir = std::env::current_dir().ok()?;
    loop {
        if dir.join("crates/tpt-t-core/Cargo.toml").exists() {
            return Some(dir);
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// Resolves `p` to an absolute path without hitting the (still-unstable on
/// Windows) `Path::absolute` helper or `canonicalize`'s `\\?\` prefix.
fn to_absolute(p: &Path) -> PathBuf {
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|c| c.join(p))
            .unwrap_or_else(|_| p.to_path_buf())
    }
}

/// Computes a relative path string from `from` to `to` (best-effort).
fn rel_path(from: &Path, to: &Path) -> Option<String> {
    let from = to_absolute(from);
    let to = to_absolute(to);
    let from_c: Vec<_> = from.components().collect();
    let to_c: Vec<_> = to.components().collect();
    let common = from_c
        .iter()
        .zip(to_c.iter())
        .take_while(|(a, b)| a == b)
        .count();
    let mut result = std::path::PathBuf::new();
    for _ in common..from_c.len() {
        result.push("..");
    }
    for c in to_c.iter().skip(common) {
        result.push(c.as_os_str());
    }
    if result.as_os_str().is_empty() {
        Some(".".into())
    } else {
        // Use forward slashes so the generated TOML is valid cross-platform
        // (TOML basic strings reject bare backslashes as escape sequences).
        Some(result.to_string_lossy().into_owned().replace('\\', "/"))
    }
}

/// Cargo.toml template. `__NAME__`/`__STRUCT__`/`__CORE__`/`__RING__`/
/// `__MACROS__` are substituted at runtime.
const CARGO_TOML: &str = "\
[package]
name = \"__NAME__\"
version = \"0.1.0\"
edition = \"2024\"
license = \"MIT OR Apache-2.0\"

[dependencies]
tpt-t-core = { path = \"__CORE__\" }
tpt-t-ring = { path = \"__RING__\" }
tpt-t-macros = { path = \"__MACROS__\" }
rkyv = { version = \"0.8\", features = [\"bytecheck\"] }

[[bin]]
name = \"__NAME__\"
path = \"src/main.rs\"
";

/// `src/main.rs` template for the scaffolded robot.
const MAIN_RS: &str = "\
//! __NAME__ — robot scaffolded by `tpt-t-cli scaffold __NAME__`.
//!
//! Replace `Camera`/`Motor` with real device types and implement
//! `RobotDevice` for each so the generated `launch()` drives them on pinned
//! cores.

use rkyv::{Archive, Serialize, Deserialize};
use tpt_t::Robot;
use tpt_t_core::profile::CoreProfile;
use tpt_t_core::robot::RobotDevice;

#[derive(Archive, Serialize, Deserialize)]
#[repr(C)]
pub struct Frame {
    pub seq: u32,
    pub pixels: [u8; 32],
}

#[derive(Archive, Serialize, Deserialize)]
#[repr(C)]
pub struct Command {
    pub throttle: f32,
}

/// Camera device: implement `run` to ingest frames and push them into the
/// generated `cam` channel.
pub struct Camera;

impl RobotDevice for Camera {
    fn run(self) {
        // TODO: capture frames, then `Self::push_cam(&channels, frame)`.
    }
}

/// Motor device: implement `run` to consume commands from the `arm` channel.
pub struct Motor;

impl RobotDevice for Motor {
    fn run(self) {
        // TODO: pull `Command`s via `Self::pop_arm(&channels)` and actuate.
    }
}

#[derive(Robot)]
#[robot(thread_per_core = true)]
pub struct __STRUCT__ {
    #[camera(id = 0, element = Frame, capacity = 256)]
    pub cam: Camera,
    #[motor(id = 1, element = Command, capacity = 256)]
    pub arm: Motor,
}

fn main() {
    let bot = __STRUCT__ { cam: Camera, arm: Motor };

    // Lock-free channels generated from the struct fields.
    let channels = bot.channels();

    // Zero-copy serialize a frame straight into a pre-allocated wire buffer.
    let mut buf = tpt_t_core::ser::AlignedBuf::new();
    let _ = __STRUCT__::serialize_cam(&Frame { seq: 1, pixels: [0; 32] }, &mut buf);
    let _ = __STRUCT__::push_cam(&channels, Frame { seq: 1, pixels: [0; 32] });
    let _ = __STRUCT__::pop_cam(&channels);

    // Pin each device to its core-profile role and run.
    let profile = CoreProfile::parse(\"video = 0\\ncontrol = 1\\n\")
        .expect(\"valid core profile\");
    let _handles = bot
        .launch(&profile)
        .expect(\"launch should pin and spawn device threads\");
    // In a real runtime, join `_handles` to await shutdown.
}
";
