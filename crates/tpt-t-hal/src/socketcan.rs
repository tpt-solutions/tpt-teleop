//! Raw SocketCAN backend (Linux; spec §5.2 "raw SocketCAN backend").
//!
//! Implements [`CanBus`] over a `AF_CAN` / `SOCK_RAW` / `CAN_RAW` socket via
//! `libc` (no `socketcan` crate — keeps the §2 MIT-only chain intact). Sockets
//! are non-blocking so `send`/`recv` honor the trait's "never block"
//! contract; a full TX queue surfaces as [`HalError::Dropped`].
//!
//! Requires a real CAN interface at runtime (e.g. a `vcan0` module); on
//! missing interfaces `open` returns [`HalError::Device`].

#![cfg(target_os = "linux")]

use std::ffi::CString;

use libc::{self, c_int, AF_CAN, CAN_RAW, O_NONBLOCK, SOCK_RAW};

use crate::can::CanBus;
use crate::types::{CanFrame, HalError};

/// CAN frame flags in the `can_id` field.
const CAN_EFF_FLAG: u32 = 0x8000_0000;

/// Open SocketCAN endpoint on `iface` (e.g. `"can0"` / `"vcan0"`).
#[derive(Debug)]
pub struct SocketCan {
    fd: c_int,
}

impl SocketCan {
    /// Opens and binds a non-blocking CAN_RAW socket to `iface`.
    pub fn open(iface: &str) -> Result<Self, HalError> {
        // SAFETY: socket(2) with a valid domain/type/proto.
        let fd = unsafe { libc::socket(AF_CAN, SOCK_RAW, CAN_RAW) };
        if fd < 0 {
            return Err(HalError::Device("socket(AF_CAN) failed"));
        }
        let name = CString::new(iface)
            .map_err(|_| HalError::Device("interface name not C-string"))?;
        // SAFETY: if_nametoindex reads the NUL-terminated name, returns an
        // index (0 on unknown interface).
        let ifindex = unsafe { libc::if_nametoindex(name.as_ptr()) };
        if ifindex == 0 {
            unsafe { libc::close(fd) };
            return Err(HalError::Device("unknown CAN interface"));
        }
        let mut addr: libc::sockaddr_can = unsafe { std::mem::zeroed() };
        addr.can_family = AF_CAN as u16;
        addr.can_ifindex = ifindex as c_int;
        // SAFETY: bind(2) over a properly-initialized sockaddr_can.
        let ret = unsafe {
            libc::bind(
                fd,
                &addr as *const _ as *const libc::sockaddr,
                std::mem::size_of::<libc::sockaddr_can>() as u32,
            )
        };
        if ret < 0 {
            unsafe { libc::close(fd) };
            return Err(HalError::Device("bind(AF_CAN) failed"));
        }
        // SAFETY: fcntl F_GETFL/F_SETFL with a valid fd; O_NONBLOCK added.
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        unsafe { libc::fcntl(fd, libc::F_SETFL, flags | O_NONBLOCK) };
        Ok(Self { fd })
    }

    /// Raw file descriptor (for integration with the Phase 2 event loop).
    pub fn as_raw_fd(&self) -> c_int {
        self.fd
    }
}

impl CanBus for SocketCan {
    fn send(&mut self, frame: &CanFrame) -> Result<(), HalError> {
        let len = frame.len.min(8) as usize;
        let id = frame.id;
        let can_id = if id > 0x7FF {
            id | CAN_EFF_FLAG
        } else {
            id
        };
        let mut cf = libc::can_frame {
            can_id,
            can_dlc: len as u8,
            __pad: 0,
            __res0: 0,
            __res1: 0,
            data: [0u8; 8],
        };
        cf.data[..len].copy_from_slice(&frame.data[..len]);
        // SAFETY: send(2) over a valid fd with a properly-sized can_frame.
        let n = unsafe {
            libc::send(
                self.fd,
                &cf as *const _ as *const libc::c_void,
                std::mem::size_of::<libc::can_frame>(),
                0,
            )
        };
        if n < 0 {
            let e = std::io::Error::last_os_error();
            if e.kind() == std::io::ErrorKind::WouldBlock {
                return Err(HalError::Dropped);
            }
            return Err(HalError::Device("CAN send failed"));
        }
        Ok(())
    }

    fn recv(&mut self, out: &mut CanFrame) -> bool {
        let mut cf: libc::can_frame = unsafe { std::mem::zeroed() };
        // SAFETY: recv(2) into a stack can_frame.
        let n = unsafe {
            libc::recv(
                self.fd,
                &mut cf as *mut _ as *mut libc::c_void,
                std::mem::size_of::<libc::can_frame>(),
                0,
            )
        };
        if n <= 0 {
            return false;
        }
        let len = (cf.can_dlc as usize).min(8);
        let mut data = [0u8; 8];
        data[..len].copy_from_slice(&cf.data[..len]);
        *out = CanFrame::new(cf.can_id & !CAN_EFF_FLAG, &data[..len]);
        true
    }
}

impl Drop for SocketCan {
    fn drop(&mut self) {
        unsafe { libc::close(self.fd) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_interface_fails_loudly() {
        // "lo" is not a CAN interface → open must error, not silently ok.
        assert!(SocketCan::open("lo").is_err());
    }
}
