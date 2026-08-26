//! Platform event loops — one abstraction, three kernels (spec §3.1).
//!
//! No async runtime: each subsystem thread drives its OS-native completion /
//! readiness facility directly:
//!
//! * Linux → [`linux::EpollLoop`] readiness loop (custom epoll, spec §3.1
//!   explicitly permits "a custom epoll event loop"; io_uring integration for
//!   network transmit lands with tpt-t-link Phase 7).
//! * macOS/BSD → [`macos::KqueueLoop`] via `kqueue(2)/kevent(2)`.
//! * Windows → [`windows::IocpLoop`] over I/O completion ports.
//!
//! User code implements [`EventHandler`] and calls [`EventLoop::dispatch`];
//! the loop never allocates per-event and never locks.

#![allow(clippy::module_inception)]

#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(windows)]
pub mod windows;

use std::io;
use std::time::Duration;

/// Opaque per-registration identifier handed back to handlers.
pub type Token = u64;

/// Readiness/completion bitset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Ready(pub u8);

impl Ready {
    /// No events.
    pub const EMPTY: Ready = Ready(0);
    /// Readable / completed receive.
    pub const READ: Ready = Ready(1);
    /// Writable / completed send.
    pub const WRITE: Ready = Ready(2);
    /// Error condition.
    pub const ERROR: Ready = Ready(4);

    /// True if any of `other`'s bits are set.
    #[inline]
    pub fn intersects(self, other: Ready) -> bool {
        self.0 & other.0 != 0
    }

    /// True if all of `other`'s bits are set.
    #[inline]
    pub fn contains(self, other: Ready) -> bool {
        self.0 & other.0 == other.0
    }
}

impl core::ops::BitOr for Ready {
    type Output = Ready;
    #[inline]
    fn bitor(self, rhs: Ready) -> Ready {
        Ready(self.0 | rhs.0)
    }
}

impl core::ops::BitOrAssign for Ready {
    #[inline]
    fn bitor_assign(&mut self, rhs: Ready) {
        self.0 |= rhs.0;
    }
}

/// What to watch. FDs on Unix; raw HANDLE addresses (as `usize`) on Windows,
/// kept address-form so the enum stays `Send + Sync`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Target {
    /// Unix file descriptor (socket, pipe, evdev node…).
    Fd(i32),
    /// Win32 HANDLE as its raw address.
    Handle(usize),
}

impl Target {
    /// Builds [`Target::Handle`] from a raw pointer-sized handle value.
    pub fn handle_raw(h: usize) -> Self {
        Target::Handle(h)
    }
}

// SAFETY: an fd/HANDLE number is just an integer identifier; validity and
// lifetime are the registrar's contract (same discipline as mio).
unsafe impl Send for Target {}
// SAFETY: shared reads of the integer identifier are harmless; registration
// APIs receive it by value.
unsafe impl Sync for Target {}

/// Consumer of readiness events.
pub trait EventHandler {
    /// Called once per ready registration. Must stay cheap; long work belongs
    /// on dedicated role threads fed by rings.
    fn ready(&mut self, token: Token, ready: Ready);
}

/// Platform event loop.
///
/// `register` may be called between dispatches; semantics are edge-triggered
/// on Unix backends (EV_CLEAR / EPOLLET-style) so handlers must drain.
pub trait EventLoop {
    /// Starts watching `target`, reporting under `token`.
    fn register(&mut self, target: Target, token: Token, interest: Ready) -> io::Result<()>;

    /// Stops watching `target` (no-op error if unknown).
    fn unregister(&mut self, target: Target) -> io::Result<()>;

    /// Waits up to `timeout` (None = forever) and feeds events to `handler`.
    /// Returns how many events were delivered.
    fn dispatch(
        &mut self,
        timeout: Option<Duration>,
        handler: &mut dyn EventHandler,
    ) -> io::Result<usize>;
}

/// The concrete backend compiled for the current platform.
#[cfg(target_os = "linux")]
pub type PlatformLoop = linux::EpollLoop;
#[cfg(target_os = "macos")]
pub type PlatformLoop = macos::KqueueLoop;
#[cfg(windows)]
pub type PlatformLoop = windows::IocpLoop;
#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
pub type PlatformLoop = stub::StubLoop;

/// Constructs the current platform's default loop.
pub fn platform_loop() -> io::Result<PlatformLoop> {
    PlatformLoop::new()
}
