//! Real-time thread priority elevation, per operating system.
//!
//! The deterministic safety loop must not compete with normal threads:
//!
//! * Linux — `SCHED_FIFO` round-robin-free realtime class (requires
//!   `CAP_SYS_NICE`; unprivileged callers get a permission error, never a
//!   panic).
//! * macOS — Mach `THREAD_TIME_CONSTRAINT_POLICY` hint (best-effort; Darwin
//!   offers no hard guarantee from userspace).
//! * Windows — thread priority `THREAD_PRIORITY_TIME_CRITICAL`.
//!
//! Elevation failures are always reported, never fatal: the loop stays
//! correct without them, just with weaker scheduling guarantees.

use std::io;

/// Priority-elevation failures.
#[derive(Debug)]
pub enum RtError {
    /// Underlying OS call failed (typically permission denied on Linux).
    Os(io::Error),
    /// Mach kernel return code (macOS).
    Mach(i32),
}

impl core::fmt::Display for RtError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            RtError::Os(e) => write!(f, "RT priority syscall failed: {e}"),
            RtError::Mach(kr) => write!(f, "mach policy failed: kern={kr}"),
        }
    }
}

impl std::error::Error for RtError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            RtError::Os(e) => Some(e),
            _ => None,
        }
    }
}

/// Elevates the **calling** thread to the strongest RT class available.
///
/// Returns `Ok(())` when the OS accepted the request. On Linux without
/// `CAP_SYS_NICE` this returns `Err(RtError::Os(EPERM))` — callers decide
/// whether that's fatal for their deployment.
pub fn elevate_current_thread() -> Result<(), RtError> {
    #[cfg(target_os = "linux")]
    {
        // SAFETY: pid 0 selects the calling thread; param fully initialized.
        unsafe {
            let mut param: libc::sched_param = std::mem::zeroed();
            param.sched_priority = libc::sched_get_priority_max(libc::SCHED_FIFO);
            if libc::sched_setscheduler(0, libc::SCHED_FIFO, &param) != 0 {
                return Err(RtError::Os(io::Error::last_os_error()));
            }
        }
        Ok(())
    }

    #[cfg(target_os = "macos")]
    {
        // THREAD_TIME_CONSTRAINT_POLICY = 2: declares a periodic computation
        // bound (50 µs work per 100 µs window) — the standard Mach RT hint.
        const THREAD_TIME_CONSTRAINT_POLICY: u32 = 2;
        const COMPUTE_TICKS: u32 = 50_000;
        const CONSTRAINT_TICKS: u32 = 100_000;
        // SAFETY: FFI with stack-resident buffer; stable Mach interface.
        unsafe {
            let port = libc::pthread_mach_thread_np(libc::pthread_self());
            let mut policy: [u32; 4] =
                [0, COMPUTE_TICKS, CONSTRAINT_TICKS, 0]; // period, compute, constraint, preemptible=false
            let kr = thread_policy_set(
                port,
                THREAD_TIME_CONSTRAINT_POLICY,
                policy.as_mut_ptr().cast::<i32>(),
                4,
            );
            if kr != 0 {
                return Err(RtError::Mach(kr));
            }
        }
        Ok(())
    }

    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Threading::{
            GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_TIME_CRITICAL,
        };
        // SAFETY: pseudo-handle valid forever; priority set is a plain call.
        unsafe {
            if SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_TIME_CRITICAL) == 0 {
                return Err(RtError::Os(io::Error::last_os_error()));
            }
        }
        Ok(())
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    Err(RtError::Unsupported("platform"))
}

#[cfg(target_os = "macos")]
unsafe extern "C" {
    /// Mach thread policy setter (<mach/thread_policy.h>).
    fn thread_policy_set(thread: u32, flavor: u32, policy_info: *mut i32, count: u32) -> i32;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elevation_reports_without_panicking() {
        // Privileged environments: Ok. Unprivileged Linux CI: EPERM error.
        // Either outcome is acceptable; what matters is no panic/abort.
        let _ = elevate_current_thread();
    }
}
