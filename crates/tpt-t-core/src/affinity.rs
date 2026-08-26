//! Thread-per-core CPU pinning (spec §3.1).
//!
//! Each latency-critical role runs on dedicated cores so the scheduler never
//! migrates it: Linux uses `sched_setaffinity`, Windows uses
//! `SetThreadAffinityMask`, and macOS applies Mach affinity/time-share policy
//! hints (best-effort — Darwin offers no hard guarantee).

use std::io;

/// Pinning failures.
#[derive(Debug)]
pub enum AffinityError {
    /// Empty core list supplied.
    NoCores,
    /// A requested core does not exist on this machine.
    OutOfRange { core: usize, max: usize },
    /// Underlying OS syscall failed.
    Os(io::Error),
    /// Mach kernel return code (macOS).
    Mach(i32),
}

impl core::fmt::Display for AffinityError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            AffinityError::NoCores => write!(f, "no cores requested"),
            AffinityError::OutOfRange { core, max } => {
                write!(f, "core {core} out of range (machine has {max})")
            }
            AffinityError::Os(e) => write!(f, "affinity syscall failed: {e}"),
            AffinityError::Mach(kr) => write!(f, "mach thread_policy_set failed: kern={kr}"),
        }
    }
}

impl std::error::Error for AffinityError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            AffinityError::Os(e) => Some(e),
            _ => None,
        }
    }
}

/// Number of logical CPUs visible to this process.
pub fn core_count() -> usize {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    // SAFETY: sysconf(_SC_NPROCESSORS_ONLN) is always safe to call.
    unsafe {
        libc::sysconf(libc::_SC_NPROCESSORS_ONLN).max(1) as usize
    }

    #[cfg(windows)]
    // SAFETY: GetSystemInfo only fills a caller-provided struct.
    unsafe {
        let mut info: windows_sys::Win32::System::SystemInformation::SYSTEM_INFO =
            std::mem::zeroed();
        windows_sys::Win32::System::SystemInformation::GetSystemInfo(&mut info);
        info.dwNumberOfProcessors.max(1) as usize
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
}

/// Pins the calling thread to run only on `cores`.
///
/// On macOS this applies Mach policy hints (advisory); on Linux/Windows it
/// is enforced by the kernel scheduler.
pub fn pin_current(cores: &[usize]) -> Result<(), AffinityError> {
    if cores.is_empty() {
        return Err(AffinityError::NoCores);
    }

    #[cfg(any(target_os = "linux", windows))]
    {
        let max = core_count();
        if let Some(&c) = cores.iter().find(|&&c| c >= max) {
            return Err(AffinityError::OutOfRange { core: c, max });
        }
    }

    #[cfg(target_os = "linux")]
    return pin_linux(cores);

    #[cfg(windows)]
    return pin_windows(cores);

    #[cfg(target_os = "macos")]
    pin_macos(cores)
}

#[cfg(target_os = "linux")]
fn pin_linux(cores: &[usize]) -> Result<(), AffinityError> {
    // SAFETY: set fully initialized before CPU_SET calls; pid 0 selects
    // the calling thread; size matches the set type.
    unsafe {
        let mut set: libc::cpu_set_t = std::mem::zeroed();
        libc::CPU_ZERO(&mut set);
        for &c in cores {
            libc::CPU_SET(c, &mut set);
        }
        if libc::sched_setaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &set) != 0 {
            return Err(AffinityError::Os(io::Error::last_os_error()));
        }
    }
    Ok(())
}

#[cfg(windows)]
fn pin_windows(cores: &[usize]) -> Result<(), AffinityError> {
    let mut mask: usize = 0;
    for &c in cores {
        mask |= 1usize << c;
    }
    // SAFETY: GetCurrentThread returns a pseudo-handle valid forever.
    unsafe {
        let h = windows_sys::Win32::System::Threading::GetCurrentThread();
        if windows_sys::Win32::System::Threading::SetThreadAffinityMask(h, mask) == 0 {
            return Err(AffinityError::Os(io::Error::last_os_error()));
        }
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn pin_macos(cores: &[usize]) -> Result<(), AffinityError> {
    // SAFETY: FFI with stack-resident buffers; Mach policy APIs are stable
    // kernel interfaces.
    unsafe {
        let port = libc::pthread_mach_thread_np(libc::pthread_self());
        // Drop out of timeshare first (RT-style priority hint).
        let mut timeshare: [i32; 1] = [0]; // 0 => not timesharing
        let kr = thread_policy_set(port, THREAD_EXTENDED_POLICY, timeshare.as_mut_ptr(), 1);
        if kr != 0 {
            return Err(AffinityError::Mach(kr));
        }
        for &c in cores {
            let mut tag: [i32; 1] = [c as i32 + 1]; // affinity tag 0 = none
            let kr = thread_policy_set(port, THREAD_AFFINITY_POLICY, tag.as_mut_ptr(), 1);
            if kr != 0 {
                return Err(AffinityError::Mach(kr));
            }
        }
    }
    Ok(())
}

// --- macOS Mach policy FFI ---------------------------------------------------

#[cfg(target_os = "macos")]
const THREAD_EXTENDED_POLICY: u32 = 1;
#[cfg(target_os = "macos")]
const THREAD_AFFINITY_POLICY: u32 = 4;

#[cfg(target_os = "macos")]
// SAFETY: declarations match <mach/thread_policy.h> exactly.
unsafe extern "C" {
    /// Mach thread policy setter (<mach/thread_policy.h>).
    fn thread_policy_set(thread: u32, flavor: u32, policy_info: *mut i32, count: u32) -> i32;
}

/// Spawns a named thread pinned to `cores`; pinning happens inside the new
/// thread before `f` runs. A failed pin panics that thread so misconfiguration
/// can never silently degrade determinism.
pub fn spawn_pinned<F>(name: &str, cores: &[usize], f: F) -> io::Result<std::thread::JoinHandle<()>>
where
    F: FnOnce() + Send + 'static,
{
    let label = format!("tpt:{name}");
    let cores = cores.to_vec();
    let name_owned = name.to_string();
    std::thread::Builder::new().name(label).spawn(move || {
        if let Err(e) = pin_current(&cores) {
            panic!("thread pinning failed for '{name_owned}' on {cores:?}: {e}");
        }
        f();
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_count_is_sane() {
        assert!(core_count() >= 1);
    }

    #[test]
    fn rejects_out_of_range_and_empty() {
        assert!(matches!(pin_current(&[]), Err(AffinityError::NoCores)));
        assert!(matches!(
            pin_current(&[core_count() + 100]),
            Err(AffinityError::OutOfRange { .. })
        ));
    }

    #[test]
    fn pinned_thread_runs_closure() {
        let h = spawn_pinned("test", &[0], || {}).expect("spawn");
        h.join().expect("pinning succeeded inside spawned thread");
    }
}
