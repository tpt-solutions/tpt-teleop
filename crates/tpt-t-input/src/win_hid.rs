//! Windows backend: raw HID reads through overlapped ReadFile
//! (`windows-sys`, `Win32_System_IO` — no hidapi crate).
//!
//! The caller supplies an already-opened file handle to the HID interface
//! (opening/enumeration is setup-API territory and stays outside the hot
//! path). Expected vendor report layout, 9 bytes:
//! `[report_id u8][buttons u16 LE][6 × axis u8 centered at 0x80]`.

use crate::report::{ControllerReport, DeviceInfo};
use crate::source::{InputError, RawInputSource};

type Handle = *mut core::ffi::c_void;
const REPORT_LEN: usize = 9;
const READ_TIMEOUT_MS: u32 = 4; // ~200 Hz loop cadence

/// Extracts `VID_xxxx` / `PID_xxxx` tokens from an interface path.
pub fn parse_u16_hex(path: &str, tag: &str) -> u16 {
    let lower = path.to_ascii_lowercase();
    let Some(i) = lower.find(&tag.to_ascii_lowercase()) else { return 0 };
    let hex: String = lower[i + tag.len()..]
        .chars()
        .take_while(|c| c.is_ascii_hexdigit())
        .collect();
    u16::from_str_radix(&hex[..hex.len().min(4)], 16).unwrap_or(0)
}

/// An opened HID interface bound to a caller-owned file handle.
#[derive(Debug)]
pub struct WinHidSource {
    handle: Handle,
    info: DeviceInfo,
}

// SAFETY: exclusive handle ownership; reads happen on the owning thread.
unsafe impl Send for WinHidSource {}

impl WinHidSource {
    /// Wraps an opened (overlapped-flagged) HID file handle.
    ///
    /// Identity is derived from the interface path when available; a blank
    /// path yields zero ids.
    pub fn from_handle(handle: Handle, path: &str) -> Result<Self, InputError> {
        if handle.is_null() || handle as isize == -1 {
            return Err(InputError::Os("invalid handle".into()));
        }
        Ok(Self { handle, info: info_from(path) })
    }
}

fn info_from(path: &str) -> DeviceInfo {
    DeviceInfo {
        vendor_id: parse_u16_hex(path, "vid_"),
        product_id: parse_u16_hex(path, "pid_"),
        path: path.to_string(),
        num_axes: 6,
        num_buttons: 16,
    }
}

impl RawInputSource for WinHidSource {
    fn poll(&mut self, out: &mut ControllerReport) -> bool {
        let mut buf = [0u8; REPORT_LEN];
        let mut ov: windows_sys::Win32::System::IO::OVERLAPPED =
            unsafe { std::mem::zeroed() };

        let mut got: u32 = 0;
        // SAFETY: buffers/overlapped are stack-resident and outlive the call.
        let ok = unsafe {
            windows_sys::Win32::Storage::FileSystem::ReadFile(
                self.handle,
                buf.as_mut_ptr(),
                buf.len() as u32,
                &mut got,
                &mut ov,
            )
        };
        if ok == 0 {
            if std::io::Error::last_os_error().raw_os_error() != Some(997) {
                return false; // ERROR_IO_PENDING expected; else idle tick
            }
            let mut transferred: u32 = 0;
            // SAFETY: bounded wait on our own overlapped operation.
            let ok2 = unsafe {
                windows_sys::Win32::System::IO::GetOverlappedResultEx(
                    self.handle,
                    &ov,
                    &mut transferred,
                    READ_TIMEOUT_MS,
                    0,
                )
            };
            if ok2 == 0 || (transferred as usize) < buf.len() {
                return false;
            }
        }

        out.seq += 1;
        out.buttons = u16::from_le_bytes([buf[1], buf[2]]) as u32;
        for (i, ax) in buf[3..9].iter().enumerate() {
            out.axes[i] = (*ax as f32 / 255.0) * 2.0 - 1.0;
        }
        out.timestamp_ns = unix_ns_now();
        true
    }

    fn info(&self) -> &DeviceInfo {
        &self.info
    }

    fn reopen(&mut self) -> Result<(), InputError> {
        Err(InputError::Unsupported(
            "reopen requires the platform opener; recreate the source",
        ))
    }
}


#[inline]
fn unix_ns_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_handle_rejected_at_construction() {
        assert!(matches!(
            WinHidSource::from_handle(std::ptr::null_mut(), "").unwrap_err(),
            InputError::Os(_)
        ));
    }

    #[test]
    fn poll_on_bogus_handle_is_idle_not_panic() {
        // A non-null, non-sentinel handle pointing at nothing: ReadFile
        // fails fast ⇒ treated as an idle tick, never a panic.
        let mut src =
            WinHidSource::from_handle(0x0000_DEAD_0000usize as Handle, r"\\?\HID#VID_045E")
                .unwrap();
        let mut rep = ControllerReport::default();
        assert!(!src.poll(&mut rep));
    }

    #[test]
    fn vid_pid_extracted_from_path_tokens() {
        assert_eq!(parse_u16_hex(r"\\?\HID#VID_045E&PID_0789", "vid_"), 0x045E);
        assert_eq!(parse_u16_hex(r"\\?\HID#VID_045E&PID_0789", "pid_"), 0x0789);
        assert_eq!(parse_u16_hex("nothing", "vid_"), 0);
    }
}
