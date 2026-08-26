//! CPU core-pinning role profiles (spec §3.1: Core 0 = video, Core 1 =
//! control, Core 2 = network …) and their dependency-free config format.
//!
//! # Config format (`core-profile.txt`)
//!
//! Plain UTF-8, one assignment per line, `#` comments, blank lines ignored:
//!
//! ```text
//! # tpt-teleop core profile
//! video   = 0
//! control = 1
//! network = 2
//! input   = 3
//! storage = 4,6
//! spare   = 5,7-9
//! ```
//!
//! Core specs may be a single index, comma lists, or inclusive ranges,
//! parsed here without serde/toml (zero-bloat policy).

use std::collections::HashMap;

/// Latency-critical roles owning dedicated cores.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Role {
    /// Video encode pipeline.
    Video,
    /// Control/safety loop.
    Control,
    /// Network I/O event loop.
    Network,
    /// HID input poller.
    Input,
    /// Media ingestion/capture.
    Media,
    /// FDR/storage writer.
    Storage,
    /// Anything unpinned / OS housekeeping.
    Spare,
}

impl Role {
    /// All roles in canonical order.
    pub const ALL: [Role; 7] = [
        Role::Video,
        Role::Control,
        Role::Network,
        Role::Input,
        Role::Media,
        Role::Storage,
        Role::Spare,
    ];

    /// Canonical lowercase key used in config files.
    pub fn key(self) -> &'static str {
        match self {
            Role::Video => "video",
            Role::Control => "control",
            Role::Network => "network",
            Role::Input => "input",
            Role::Media => "media",
            Role::Storage => "storage",
            Role::Spare => "spare",
        }
    }

    /// Parses a role key (case-insensitive); `None` when unknown.
    pub fn from_key(s: &str) -> Option<Role> {
        let lower = s.to_ascii_lowercase();
        Role::ALL.into_iter().find(|r| r.key() == lower)
    }
}

impl core::fmt::Display for Role {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.key())
    }
}

/// One bad line in a profile config.
#[derive(Debug)]
pub struct ProfileError {
    /// 1-based line number.
    pub lineno: usize,
    /// What was wrong.
    pub kind: LineKind,
}

/// Line-level error details.
#[derive(Debug)]
pub enum LineKind {
    /// No `=` separator.
    MissingEquals(String),
    /// Key is not a known [`Role`].
    UnknownRole(String),
    /// Core spec could not be parsed.
    BadCoreSpec(String),
    /// Same role assigned twice.
    DuplicateRole(String),
}

impl core::fmt::Display for ProfileError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "core-profile line {}: ", self.lineno)?;
        match &self.kind {
            LineKind::MissingEquals(s) => write!(f, "expected `key = value`, got {s:?}"),
            LineKind::UnknownRole(s) => write!(f, "unknown role {s:?}"),
            LineKind::BadCoreSpec(s) => write!(f, "bad core spec {s}"),
            LineKind::DuplicateRole(s) => write!(f, "duplicate assignment for {s:?}"),
        }
    }
}

impl std::error::Error for ProfileError {}

/// A validated mapping of roles to core sets.
#[derive(Debug, Clone, Default)]
pub struct CoreProfile {
    assigned: HashMap<&'static str, Vec<usize>>,
}

impl CoreProfile {
    /// Parses config text. Unknown keys are hard errors (typo-proofing).
    pub fn parse(text: &str) -> Result<Self, ProfileError> {
        let mut assigned = HashMap::new();
        for (lineno, raw) in text.lines().enumerate() {
            let line = raw.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let lineno = lineno + 1;
            let bad = |kind: LineKind| ProfileError { lineno, kind };
            let (key, value) = line
                .split_once('=')
                .ok_or_else(|| bad(LineKind::MissingEquals(raw.into())))?;
            let key = key.trim();
            let role =
                Role::from_key(key).ok_or_else(|| bad(LineKind::UnknownRole(key.to_string())))?;
            let cores = parse_core_list(value.trim()).map_err(|s| bad(LineKind::BadCoreSpec(s)))?;
            if cores.is_empty() {
                return Err(bad(LineKind::BadCoreSpec("empty list".into())));
            }
            if assigned.insert(role.key(), cores.clone()).is_some() {
                return Err(bad(LineKind::DuplicateRole(key.to_string())));
            }
        }
        Ok(Self { assigned })
    }

    /// Cores assigned to `role`; empty slice when unset.
    pub fn cores_for(&self, role: Role) -> &[usize] {
        self.assigned
            .get(role.key())
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// True when every listed core exists on this machine and no core is
    /// claimed twice across roles (double-pinning would thrash caches).
    pub fn validate(&self, machine_cores: usize) -> Result<(), String> {
        let mut seen = vec![false; machine_cores];
        for role in Role::ALL {
            for &c in self.cores_for(role) {
                if c >= machine_cores {
                    return Err(format!("{role} requests core {c} >= {machine_cores}"));
                }
                if seen[c] {
                    return Err(format!("core {c} assigned multiple times ({role})"));
                }
                seen[c] = true;
            }
        }
        Ok(())
    }
}

/// Loads and parses a profile file.
pub fn load_profile(path: impl AsRef<Path>) -> Result<CoreProfile, ProfileLoadError> {
    let path = path.as_ref();
    let text = std::fs::read_to_string(path)
        .map_err(|e| ProfileLoadError(path.display().to_string(), e))?;
    CoreProfile::parse(&text).map_err(|e| {
        ProfileLoadError(
            path.display().to_string(),
            std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()),
        )
    })
}

/// File-level failure wrapper.
#[derive(Debug)]
pub struct ProfileLoadError(pub String, pub std::io::Error);

impl core::fmt::Display for ProfileLoadError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "cannot load core profile {}: {}", self.0, self.1)
    }
}

impl std::error::Error for ProfileLoadError {}

fn parse_core_list(s: &str) -> Result<Vec<usize>, String> {
    let mut out = Vec::new();
    for part in s.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if let Some((a, b)) = part.split_once('-') {
            let a: usize = a.trim().parse().map_err(|_| format!("range {part:?}"))?;
            let b: usize = b.trim().parse().map_err(|_| format!("range {part:?}"))?;
            if b < a || b - a > 4096 {
                return Err(format!("range {part:?}"));
            }
            out.extend(a..=b);
        } else {
            out.push(
                part.parse::<usize>()
                    .map_err(|_| format!("index {part:?}"))?,
            );
        }
    }
    Ok(out)
}

use std::path::Path;
