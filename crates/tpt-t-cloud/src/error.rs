//! Crate-wide error type.

use std::io;
use std::net::SocketAddr;

use tpt_t_core::mode::Mode;

/// Errors surfaced by the cloud/fleet stack.
#[derive(Debug)]
pub enum CloudError {
    /// Underlying I/O failure (socket, file, event loop).
    Io(io::Error),
    /// JSON (de)serialization failure.
    Json(String),
    /// HTTP request parsing failure.
    Http(String),
    /// A unit id is not known to the fleet.
    NotFound(u64),
    /// A unit id is already provisioned.
    AlreadyExists(u64),
    /// A requested mode transition is forbidden by the transition table.
    ModeDisallowed { from: Mode, to: Mode },
    /// The transport rejected a command send.
    Transport(String),
    /// The recorder rejected a write.
    Recorder(String),
    /// A capability is unavailable under the current dependency policy.
    Unsupported(&'static str),
}

impl core::fmt::Display for CloudError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            CloudError::Io(e) => write!(f, "io error: {e}"),
            CloudError::Json(e) => write!(f, "json error: {e}"),
            CloudError::Http(e) => write!(f, "http error: {e}"),
            CloudError::NotFound(id) => write!(f, "unit {id} not found"),
            CloudError::AlreadyExists(id) => write!(f, "unit {id} already exists"),
            CloudError::ModeDisallowed { from, to } => {
                write!(
                    f,
                    "mode transition {} -> {} not permitted",
                    from.name(),
                    to.name()
                )
            }
            CloudError::Transport(e) => write!(f, "transport error: {e}"),
            CloudError::Recorder(e) => write!(f, "recorder error: {e}"),
            CloudError::Unsupported(msg) => write!(f, "unsupported: {msg}"),
        }
    }
}

impl std::error::Error for CloudError {}

impl From<io::Error> for CloudError {
    fn from(e: io::Error) -> Self {
        CloudError::Io(e)
    }
}

impl From<CloudError> for io::Error {
    fn from(e: CloudError) -> Self {
        match e {
            CloudError::Io(e) => e,
            other => io::Error::other(other.to_string()),
        }
    }
}

/// Convenience: format a transport send failure from an address + error.
pub(crate) fn transport_err(addr: SocketAddr, e: io::Error) -> CloudError {
    CloudError::Transport(format!("{addr}: {e}"))
}
