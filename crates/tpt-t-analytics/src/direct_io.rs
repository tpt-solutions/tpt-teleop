//! Direct-I/O file abstraction that bypasses the OS page cache so FDR writes
//! never stall behind dirty-page writeback (spec §5.8 "O_DIRECT Logging").
//!
//! Every platform backend writes in fixed [`SECTOR_SIZE`]-aligned blocks (both
//! the buffer address and the length are multiples of the sector size, and the
//! file offset advances in sector multiples) — the three hard requirements of
//! OS direct I/O. A small aligned staging buffer accumulates caller bytes and
//! flushes whole sectors; the trailing partial sector is zero-padded and
//! flushed on [`DirectFile::flush`].
//!
//! If a platform's direct-I/O open fails (e.g. `O_DIRECT` on a `tmpfs` mount
//! that rejects it), the backend transparently falls back to a buffered file
//! and reports `is_direct() == false` so callers can observe the degradation.

use std::alloc::{self, Layout};
use std::io;
use std::path::Path;
use std::slice;

/// Sector alignment / length multiple for direct-I/O writes. 4096 is a
/// superset of every common block-device logical sector (512) so a single
/// constant satisfies Linux `O_DIRECT`, Windows `FILE_FLAG_NO_BUFFERING`, and
/// macOS `F_NOCACHE`.
pub const SECTOR_SIZE: usize = 4096;

/// Aligned, fixed-capacity scratch buffer (capacity is a multiple of
/// [`SECTOR_SIZE`]).
struct AlignedBuf {
    ptr: *mut u8,
    cap: usize,
}

// SAFETY: ptr is allocated with the matching Layout and freed identically.
unsafe impl Send for AlignedBuf {}
// SAFETY: &AlignedBuf only yields shared slice views; the pointee is plain u8.
unsafe impl Sync for AlignedBuf {}

impl AlignedBuf {
    fn new(cap: usize) -> Self {
        assert!(
            cap > 0 && cap % SECTOR_SIZE == 0,
            "cap must be a sector multiple"
        );
        let layout = Layout::from_size_align(cap, SECTOR_SIZE).expect("valid layout");
        // SAFETY: `layout` has non-zero size and a power-of-two alignment, so
        // alloc is sound; we reject a null result below.
        let ptr = unsafe { alloc::alloc(layout) };
        assert!(!ptr.is_null(), "aligned allocation failed");
        Self { ptr, cap }
    }

    fn cap(&self) -> usize {
        self.cap
    }
}

impl Drop for AlignedBuf {
    fn drop(&mut self) {
        let layout = Layout::from_size_align(self.cap, SECTOR_SIZE).expect("valid layout");
        // SAFETY: `layout` matches the one used in `new` for this allocation,
        // and this runs exactly once at drop.
        unsafe { alloc::dealloc(self.ptr, layout) };
    }
}

impl std::ops::Deref for AlignedBuf {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        // SAFETY: ptr points at `cap` initialized-or-zeroed u8 bytes.
        unsafe { slice::from_raw_parts(self.ptr, self.cap) }
    }
}

impl std::ops::DerefMut for AlignedBuf {
    fn deref_mut(&mut self) -> &mut [u8] {
        // SAFETY: as Deref, plus exclusive borrow.
        unsafe { slice::from_raw_parts_mut(self.ptr, self.cap) }
    }
}

#[cfg(unix)]
mod imp {
    use std::ffi::CString;
    use std::io;
    use std::os::raw::c_void;
    use std::path::Path;

    use libc;

    pub struct RawFile {
        pub fd: i32,
    }

    // SAFETY: an fd is just an integer handle; moving it across threads is
    // sound (the kernel object is referenced by number).
    unsafe impl Send for RawFile {}

    #[cfg(target_os = "linux")]
    const O_DIRECT_FLAGS: i32 = libc::O_DIRECT;
    #[cfg(target_os = "macos")]
    const O_DIRECT_FLAGS: i32 = 0;

    impl RawFile {
        pub fn open(path: &Path) -> io::Result<(Self, bool)> {
            let c = CString::new(path.as_os_str().to_string_lossy().as_bytes()).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidInput, "path is not C-string safe")
            })?;

            // Preferred: open with direct I/O (O_DIRECT on Linux; no-op flag on
            // macOS where we instead use fcntl(F_NOCACHE) below).
            let direct = unsafe {
                libc::open(
                    c.as_ptr(),
                    libc::O_WRONLY | libc::O_CREAT | libc::O_TRUNC | O_DIRECT_FLAGS,
                    0o644,
                )
            };
            if direct >= 0 {
                #[cfg(target_os = "macos")]
                // SAFETY: valid fd; F_NOCACHE=1 bypasses the page cache.
                unsafe {
                    libc::fcntl(direct, libc::F_NOCACHE, 1);
                }
                return Ok((RawFile { fd: direct }, true));
            }

            // Fallback: some filesystems reject O_DIRECT. Retry buffered so the
            // recorder still works (e.g. on CI tmpfs); report degraded mode.
            let buffered = unsafe {
                libc::open(
                    c.as_ptr(),
                    libc::O_WRONLY | libc::O_CREAT | libc::O_TRUNC,
                    0o644,
                )
            };
            if buffered < 0 {
                return Err(io::Error::last_os_error());
            }
            #[cfg(target_os = "macos")]
            // SAFETY: valid fd; F_NOCACHE still bypasses cache on the buffered fd.
            unsafe {
                libc::fcntl(buffered, libc::F_NOCACHE, 1);
            }
            Ok((RawFile { fd: buffered }, false))
        }

        /// Writes exactly `block.len()` bytes (must be a sector multiple,
        /// buffer sector-aligned) to the current file offset.
        pub fn write_block(&self, block: &[u8]) -> io::Result<()> {
            let mut off = 0;
            while off < block.len() {
                // SAFETY: block[off..] is valid; write takes a raw pointer and
                // count of initialized bytes.
                let n = unsafe {
                    libc::write(
                        self.fd,
                        block[off..].as_ptr() as *const c_void,
                        block.len() - off,
                    )
                };
                if n < 0 {
                    return Err(io::Error::last_os_error());
                }
                off += n as usize;
            }
            Ok(())
        }

        pub fn flush(&self) -> io::Result<()> {
            // SAFETY: valid fd; fsync is best-effort durability.
            unsafe {
                libc::fsync(self.fd);
            }
            Ok(())
        }
    }

    impl Drop for RawFile {
        fn drop(&mut self) {
            // SAFETY: closing our own descriptor exactly once.
            unsafe {
                libc::close(self.fd);
            }
        }
    }
}

#[cfg(windows)]
mod imp {
    use std::ffi::CString;
    use std::io;
    use std::path::Path;

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        CREATE_ALWAYS, CreateFileA, FILE_FLAG_NO_BUFFERING, FILE_FLAG_WRITE_THROUGH,
        FILE_WRITE_ATTRIBUTES, FILE_WRITE_DATA, SYNCHRONIZE, WriteFile,
    };

    pub struct RawFile {
        pub handle: HANDLE,
    }

    // SAFETY: a Win32 HANDLE is a process-local integer-like token; sending it
    // to another thread of the same process is sound.
    unsafe impl Send for RawFile {}

    impl RawFile {
        pub fn open(path: &Path) -> io::Result<(Self, bool)> {
            let c = CString::new(path.as_os_str().to_string_lossy().as_bytes()).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidInput, "path is not C-string safe")
            })?;
            // SAFETY: valid C path pointer; no template/security attrs; flags
            // request unbuffered, write-through direct I/O.
            let handle = unsafe {
                CreateFileA(
                    c.as_ptr() as *const u8,
                    FILE_WRITE_DATA | FILE_WRITE_ATTRIBUTES | SYNCHRONIZE,
                    0,
                    std::ptr::null_mut(),
                    CREATE_ALWAYS,
                    FILE_FLAG_NO_BUFFERING | FILE_FLAG_WRITE_THROUGH,
                    std::ptr::null_mut(),
                )
            };
            if handle == INVALID_HANDLE_VALUE {
                return Err(io::Error::last_os_error());
            }
            Ok((RawFile { handle }, true))
        }

        pub fn write_block(&self, block: &[u8]) -> io::Result<()> {
            let mut written: u32 = 0;
            // SAFETY: block is valid for `block.len()` bytes; written is a stack
            // out-pointer; no OVERLAPPED (synchronous write).
            let ok = unsafe {
                WriteFile(
                    self.handle,
                    block.as_ptr(),
                    block.len() as u32,
                    &mut written,
                    std::ptr::null_mut(),
                )
            };
            if ok == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        }

        pub fn flush(&self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Drop for RawFile {
        fn drop(&mut self) {
            // SAFETY: closing our own handle exactly once.
            unsafe {
                CloseHandle(self.handle);
            }
        }
    }
}

/// A direct-I/O file. Bytes handed to [`DirectFile::write`] are staged in an
/// aligned buffer and pushed to the kernel in [`SECTOR_SIZE`] blocks.
///
/// The file is opened for writing and truncated. On close (or
/// [`DirectFile::flush`]) the trailing partial sector is zero-padded to a full
/// sector so every `write_block` satisfies the direct-I/O alignment rules.
pub struct DirectFile {
    backend: imp::RawFile,
    staging: AlignedBuf,
    staging_len: usize,
    direct: bool,
    written: u64,
}

impl DirectFile {
    /// Opens `path` for direct-I/O writing, creating/truncating it.
    pub fn open(path: &Path) -> io::Result<Self> {
        let (backend, direct) = imp::RawFile::open(path)?;
        Ok(Self {
            backend,
            staging: AlignedBuf::new(SECTOR_SIZE * 16),
            staging_len: 0,
            direct,
            written: 0,
        })
    }

    /// True when the OS direct-I/O path is actually in use (false if we fell
    /// back to a buffered file because `O_DIRECT`/equivalent was unavailable).
    pub fn is_direct(&self) -> bool {
        self.direct
    }

    /// Total bytes handed to the kernel so far (sector-multiple writes only).
    pub fn written(&self) -> u64 {
        self.written
    }

    /// Appends `data`, flushing whole sectors to the kernel as the staging
    /// buffer fills. Never blocks behind the page cache.
    pub fn write(&mut self, mut data: &[u8]) -> io::Result<()> {
        while !data.is_empty() {
            let space = self.staging.cap() - self.staging_len;
            let take = space.min(data.len());
            self.staging[self.staging_len..self.staging_len + take].copy_from_slice(&data[..take]);
            self.staging_len += take;
            data = &data[take..];
            while self.staging_len >= SECTOR_SIZE {
                self.backend.write_block(&self.staging[..SECTOR_SIZE])?;
                let rem = self.staging_len - SECTOR_SIZE;
                self.staging.copy_within(SECTOR_SIZE..self.staging_len, 0);
                self.staging_len = rem;
                self.written += SECTOR_SIZE as u64;
            }
        }
        Ok(())
    }

    /// Flushes the trailing partial sector (zero-padded) and the backend.
    pub fn flush(&mut self) -> io::Result<()> {
        if self.staging_len > 0 {
            for b in &mut self.staging[self.staging_len..SECTOR_SIZE] {
                *b = 0;
            }
            // `staging` is SECTOR_SIZE-aligned, so writing its leading sector
            // satisfies the direct-I/O alignment rules (a stack array would
            // not be aligned and WriteFile would fail with ERROR_INVALID_PARAMETER).
            self.backend.write_block(&self.staging[..SECTOR_SIZE])?;
            self.written += SECTOR_SIZE as u64;
            self.staging_len = 0;
        }
        self.backend.flush()?;
        Ok(())
    }
}
