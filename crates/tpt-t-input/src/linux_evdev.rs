//! Linux backend: custom evdev bindings over raw syscalls (`libc` only —
//! no hidapi/evdev crates, per spec §5.1 "custom evdev bindings").

use std::ffi::CString;

use super::evdev_parse::{EVENT_SIZE, EvdevAccumulator, decode_event};
use crate::report::{ControllerReport, DeviceInfo};
use crate::source::{InputError, RawInputSource};

/// Kernel uapi request builder (`_IOR('E', nr, size)`), asm-generic/ioctl.h.
///
/// `libc::ioctl`'s request parameter is `u64` on modern linux-gnu targets
/// (it mirrors the kernel's `unsigned long`), so requests are built in u64.
const fn ev_ior(nr: u32, size: usize) -> u64 {
    (2u32 << 30) as u64 | ((size & 0x3FFF) as u64) << 16 | (b'E' as u64) << 8 | nr as u64
}

/// `EVIOCGID` — device id probe.
fn eviocg_id() -> u64 {
    ev_ior(0x02, std::mem::size_of::<[u16; 4]>())
}

/// `EVIOCGABS(axis)` — absolute-axis calibration probe.
fn eviocg_abs(code: u32) -> u64 {
    ev_ior(0x40 + code, std::mem::size_of::<InputAbsInfo>())
}

/// Mirror of `struct input_absinfo`.
#[repr(C)]
#[derive(Default)]
struct InputAbsInfo {
    value: i32,
    minimum: i32,
    maximum: i32,
    fuzz: i32,
    flat: i32,
    resolution: i32,
}

/// Mirror of `struct input_id`.
#[repr(C)]
#[derive(Default)]
struct InputId {
    bustype: u16,
    vendor: u16,
    product: u16,
    version: u16,
}

/// Axis codes calibrated at open time, in slot order
/// `[roll, pitch, yaw, throttle, lat_x, lat_y, spare0, spare1]`.
const CAL_CODES: [u16; 8] = [
    super::evdev_parse::ABS_X,
    super::evdev_parse::ABS_Y,
    super::evdev_parse::ABS_RX,
    super::evdev_parse::ABS_THROTTLE,
    super::evdev_parse::ABS_RUDDER,
    super::evdev_parse::ABS_HAT0X,
    super::evdev_parse::ABS_Z,
    super::evdev_parse::ABS_RZ,
];

/// An opened evdev device node.
#[derive(Debug)]
pub struct EvdevSource {
    fd: i32,
    info: DeviceInfo,
    acc: EvdevAccumulator,
}

// SAFETY: unique fd ownership; all state lives on the owning thread.
unsafe impl Send for EvdevSource {}

impl EvdevSource {
    /// Opens `path` non-blocking and calibrates axes via `EVIOCGABS`.
    pub fn open(path: &str) -> Result<Self, InputError> {
        let c = CString::new(path).map_err(|_| InputError::BadPath("nul in path"))?;
        // SAFETY: path is a valid NUL-terminated C string.
        let fd = unsafe {
            libc::open(
                c.as_ptr(),
                libc::O_RDONLY | libc::O_NONBLOCK | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(InputError::Os(std::io::Error::last_os_error().to_string()));
        }

        // SAFETY: fixed-size ioctls into stack structs on our own fd.
        let (vid, pid) = unsafe {
            let mut id = InputId::default();
            if libc::ioctl(fd, eviocg_id(), &mut id as *mut InputId) >= 0 {
                (id.vendor, id.product)
            } else {
                (0, 0)
            }
        };

        let mut acc = EvdevAccumulator::default();
        // SAFETY: fixed-size EVIOCGABS ioctls into a stack InputAbsInfo on
        // our own read-only fd; failure leaves the default calibration.
        unsafe {
            for (slot_i, &code) in CAL_CODES.iter().enumerate() {
                let mut ai = InputAbsInfo::default();
                if libc::ioctl(fd, eviocg_abs(code as u32), &mut ai as *mut InputAbsInfo) == 0 {
                    acc.calib[slot_i.min(7)] = (ai.minimum, ai.maximum);
                }
            }
        }

        Ok(Self {
            fd,
            info: DeviceInfo {
                vendor_id: vid,
                product_id: pid,
                path: path.to_string(),
                num_axes: CAL_CODES.len().min(255) as u8,
                num_buttons: 64,
            },
            acc,
        })
    }

    /// Drains all currently readable events; `true` when any arrived.
    fn drain(&mut self) -> bool {
        let mut buf = [0u8; 64 * EVENT_SIZE];
        let mut got_any = false;
        loop {
            // SAFETY: read into our stack buffer; fd owned by self.
            let n = unsafe { libc::read(self.fd, buf.as_mut_ptr().cast(), buf.len()) };
            if n <= 0 {
                break; // EAGAIN ⇒ drained; transient errors surface next tick
            }
            let n = n as usize;
            let mut off = 0usize;
            while off + EVENT_SIZE <= n {
                if let Some(ev) = decode_event(&buf, off) {
                    got_any |= self.acc.push(&ev);
                }
                off += EVENT_SIZE;
            }
            if n < buf.len() {
                break;
            }
        }
        got_any
    }
}

impl Drop for EvdevSource {
    fn drop(&mut self) {
        // SAFETY: closing our own descriptor exactly once.
        unsafe { libc::close(self.fd) };
    }
}

impl RawInputSource for EvdevSource {
    fn poll(&mut self, out: &mut ControllerReport) -> bool {
        if !self.drain() {
            return false;
        }
        self.acc.snapshot(out);
        out.seq = out.seq.wrapping_add(1);
        true
    }

    fn info(&self) -> &DeviceInfo {
        &self.info
    }

    fn reopen(&mut self) -> Result<(), InputError> {
        let path = self.info.path.clone();
        *self = Self::open(&path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opening_a_missing_node_is_a_clean_error() {
        let err = EvdevSource::open("/dev/input/nonexistent-tpt").unwrap_err();
        assert!(matches!(err, InputError::Os(_)));
    }
}
