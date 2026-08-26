//! Windows backend: I/O completion ports via `windows-sys`.

use std::collections::HashSet;
use std::io;
use std::time::Duration;

use windows_sys::Win32::Foundation::{INVALID_HANDLE_VALUE, WAIT_TIMEOUT};
use windows_sys::Win32::System::IO::{
    CreateIoCompletionPort, GetQueuedCompletionStatus, PostQueuedCompletionStatus,
};

use super::{EventHandler, EventLoop, Ready, Target, Token};

type HandlePtr = *mut core::ffi::c_void;

/// IOCP-backed loop. IOCP carries no read/write distinction — every
/// completion reports both [`Ready::READ`] and [`Ready::WRITE`]. Synthetic
/// wakeups are available through [`IocpLoop::post_wakeup`].
pub struct IocpLoop {
    port: HandlePtr,
    registered: HashSet<usize>,
}

// SAFETY: the completion port handle is exclusively owned by this struct;
// registration keys flow back verbatim. Handlers run on owning thread.
unsafe impl Send for IocpLoop {}
// SAFETY: &IocpLoop only forwards the stored handle into syscalls by value.
unsafe impl Sync for IocpLoop {}

impl core::fmt::Debug for IocpLoop {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("IocpLoop")
            .field("registered", &self.registered.len())
            .finish()
    }
}

impl IocpLoop {
    /// Creates a new completion port.
    pub fn new() -> io::Result<Self> {
        // SAFETY: creating a fresh port with no source handle.
        let port =
            unsafe { CreateIoCompletionPort(INVALID_HANDLE_VALUE, std::ptr::null_mut(), 0, 0) };
        if port.is_null() {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            port,
            registered: HashSet::new(),
        })
    }

    /// Posts a synthetic completion for `token` (timers, cross-thread kicks).
    pub fn post_wakeup(&self, token: Token) -> io::Result<()> {
        // SAFETY: valid port handle; null OVERLAPPED marks synthetic events.
        let ok = unsafe {
            PostQueuedCompletionStatus(self.port, 0, token as usize, std::ptr::null_mut())
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

impl EventLoop for IocpLoop {
    fn register(&mut self, target: Target, token: Token, _interest: Ready) -> io::Result<()> {
        let handle: HandlePtr = match target {
            Target::Handle(h) => h as HandlePtr,
            Target::Fd(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "fds are Unix-only",
                ));
            }
        };
        // SAFETY: associating a caller-owned handle with our port; the key
        // round-trips through GetQueuedCompletionStatus verbatim.
        let p = unsafe { CreateIoCompletionPort(handle, self.port, token as usize, 0) };
        if p.is_null() {
            return Err(io::Error::last_os_error());
        }
        self.registered.insert(handle as usize);
        Ok(())
    }

    fn unregister(&mut self, target: Target) -> io::Result<()> {
        // Windows cannot detach a handle from a port; we drop bookkeeping so
        // late completions are ignored by dispatch filtering below.
        if let Target::Handle(h) = target {
            self.registered.remove(&h);
        }
        Ok(())
    }

    fn dispatch(
        &mut self,
        timeout: Option<Duration>,
        handler: &mut dyn EventHandler,
    ) -> io::Result<usize> {
        let ms = timeout
            .map(|d| d.as_millis().min(u32::MAX as u128) as u32)
            .unwrap_or(u32::MAX);
        let mut bytes: u32 = 0;
        let mut key: usize = 0;
        let mut overlapped: *mut windows_sys::Win32::System::IO::OVERLAPPED = std::ptr::null_mut();
        // SAFETY: all out-pointers reference stack locals; port owned by us.
        let ok = unsafe {
            GetQueuedCompletionStatus(self.port, &mut bytes, &mut key, &mut overlapped, ms)
        };
        if ok == 0 {
            let err = io::Error::last_os_error();
            if err.raw_os_error() == Some(WAIT_TIMEOUT as i32) {
                return Ok(0);
            }
            return Err(err);
        }
        handler.ready(key as Token, Ready::READ | Ready::WRITE);
        Ok(1)
    }
}
