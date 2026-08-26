//! Direct memory-mapped I/O for CAN/serial register blocks (spec §5.2
//! "direct memory-mapped I/O for CAN bus and serial").
//!
//! [`Mmio`] is the trait every backend implements. [`BufferMmio`] is a
//! sim-backed window (used by the Phase 4 simulator and tests); the real
//! [`LinuxMmio`] maps physical memory through `/dev/mem` on Linux. Both expose
//! volatile 32-bit register access so register writes are never elided.

/// A memory-mapped register window.
#[cfg(target_os = "linux")]
use crate::types::HalError;

pub trait Mmio {
    /// Reads a 32-bit register at byte `offset` (must be 4-aligned).
    fn read_u32(&self, offset: usize) -> u32;
    /// Writes a 32-bit register at byte `offset`.
    fn write_u32(&mut self, offset: usize, value: u32);
}

/// Sim-backed MMIO over a plain buffer.
#[derive(Debug)]
pub struct BufferMmio {
    mem: Vec<u8>,
}

impl BufferMmio {
    /// Creates a `size`-byte register window, zero-initialized.
    pub fn new(size: usize) -> Self {
        Self {
            mem: vec![0u8; size],
        }
    }
}

impl Mmio for BufferMmio {
    fn read_u32(&self, offset: usize) -> u32 {
        u32::from_le_bytes([
            self.mem[offset],
            self.mem[offset + 1],
            self.mem[offset + 2],
            self.mem[offset + 3],
        ])
    }
    fn write_u32(&mut self, offset: usize, value: u32) {
        self.mem[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
}

/// Real memory-mapped register window (Linux; via `/dev/mem`).
///
/// Deferred to hardware bring-up in CI, but the mapping is wired so a unit
/// with mmio CAN/serial IP can drive registers directly. `open` maps
/// `size` bytes at physical `phys_addr` and exposes volatile access.
#[cfg(target_os = "linux")]
pub struct LinuxMmio {
    ptr: *mut u8,
    size: usize,
    fd: std::os::fd::RawFd,
}

#[cfg(target_os = "linux")]
unsafe impl Send for LinuxMmio {}

#[cfg(target_os = "linux")]
impl LinuxMmio {
    /// Maps `size` bytes of physical memory at `phys_addr`.
    pub fn open(phys_addr: usize, size: usize) -> Result<Self, HalError> {
        use std::os::fd::AsRawFd;
        // SAFETY: open(2) of /dev/mem; needs CAP_SYS_RAWIO at runtime.
        let fd = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/mem");
        let file = match fd {
            Ok(f) => f,
            Err(_) => return Err(HalError::Device("cannot open /dev/mem")),
        };
        let raw = file.as_raw_fd();
        // SAFETY: mmap(2) of a device file; the returned region is
        // MAP_SHARED and must be munmapped on drop.
        let ptr = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                size,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                raw,
                phys_addr as libc::off_t,
            )
        };
        if ptr == libc::MAP_FAILED {
            return Err(HalError::Device("mmap of physical memory failed"));
        }
        Ok(Self {
            ptr: ptr as *mut u8,
            size,
            fd: raw,
        })
    }
}

#[cfg(target_os = "linux")]
impl Mmio for LinuxMmio {
    fn read_u32(&self, offset: usize) -> u32 {
        assert!(offset + 4 <= self.size, "MMIO read out of range");
        // SAFETY: offset is bounds-checked; volatile read prevents elision.
        unsafe { std::ptr::read_volatile(self.ptr.add(offset) as *const u32) }
    }
    fn write_u32(&mut self, offset: usize, value: u32) {
        assert!(offset + 4 <= self.size, "MMIO write out of range");
        // SAFETY: offset is bounds-checked; volatile write prevents elision.
        unsafe { std::ptr::write_volatile(self.ptr.add(offset) as *mut u32, value) }
    }
}

#[cfg(target_os = "linux")]
impl Drop for LinuxMmio {
    fn drop(&mut self) {
        unsafe {
            libc::munmap(self.ptr as *mut libc::c_void, self.size);
            libc::close(self.fd);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffer_mmio_reads_what_it_writes() {
        let mut m = BufferMmio::new(64);
        m.write_u32(0, 0xDEAD_BEEF);
        m.write_u32(16, 0x1234_5678);
        assert_eq!(m.read_u32(0), 0xDEAD_BEEF);
        assert_eq!(m.read_u32(16), 0x1234_5678);
        // Unwritten region is zeroed.
        assert_eq!(m.read_u32(32), 0);
    }
}
