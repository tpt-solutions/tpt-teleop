//! macOS/BSD backend: `kqueue`/`kevent` readiness via `libc`.

use std::collections::HashMap;
use std::io;
use std::time::Duration;

use super::{EventHandler, EventLoop, Ready, Target, Token};

const KQ_EV_ADD: u16 = 0x0001;
const KQ_EV_DELETE: u16 = 0x0002;
const KQ_EV_ENABLE: u16 = 0x0004;
const KQ_EV_CLEAR: u16 = 0x0020;
const KQ_EV_EOF: u16 = 0x8000;
const KQ_EV_ERROR: u16 = 0x4000;
const KQ_EVFILT_READ: i16 = -1;
const KQ_EVFILT_WRITE: i16 = -2;

/// kqueue-backed readiness loop (edge-triggered via EV_CLEAR).
#[derive(Debug)]
pub struct KqueueLoop {
    kq: i32,
    regs: HashMap<Token, (i32, Vec<i16>)>, // token -> (fd, filters)
}

// SAFETY: unique kq ownership; handlers run on the owning thread only.
unsafe impl Send for KqueueLoop {}
// SAFETY: &KqueueLoop forwards the owned kq descriptor into syscalls by
// value; registration state is plain bookkeeping.
unsafe impl Sync for KqueueLoop {}

impl KqueueLoop {
    /// Creates the kernel queue.
    pub fn new() -> io::Result<Self> {
        // SAFETY: plain syscall without pointer arguments.
        let kq = unsafe { libc::kqueue() };
        if kq < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            kq,
            regs: HashMap::new(),
        })
    }

    fn arm(&mut self, fd: i32, token: Token, interest: Ready) -> io::Result<()> {
        let mut filters = Vec::with_capacity(2);
        if interest.intersects(Ready::READ) {
            filters.push(KQ_EVFILT_READ);
        }
        if interest.intersects(Ready::WRITE) {
            filters.push(KQ_EVFILT_WRITE);
        }
        // SAFETY: single-element changelists of fully-initialized structs
        // against our own kqueue descriptor.
        unsafe {
            for filter in &filters {
                let ev = libc::kevent {
                    ident: fd as usize,
                    filter: *filter,
                    flags: KQ_EV_ADD | KQ_EV_ENABLE | KQ_EV_CLEAR,
                    fflags: 0,
                    data: 0,
                    udata: token as usize as *mut core::ffi::c_void,
                };
                let n = libc::kevent(self.kq, &ev, 1, std::ptr::null_mut(), 0, std::ptr::null());
                if n < 0 {
                    return Err(io::Error::last_os_error());
                }
            }
        }
        self.regs.insert(token, (fd, filters));
        Ok(())
    }

    fn disarm_fd(&mut self, fd: i32) -> io::Result<()> {
        let Some(token) = self
            .regs
            .iter()
            .find(|(_, (f, _))| *f == fd)
            .map(|(t, _)| *t)
        else {
            return Ok(());
        };
        let (_, filters) = self.regs.remove(&token).unwrap();
        // SAFETY: deletion entries for previously-added filters only.
        unsafe {
            for filter in filters {
                let ev = libc::kevent {
                    ident: fd as usize,
                    filter,
                    flags: KQ_EV_DELETE,
                    fflags: 0,
                    data: 0,
                    udata: std::ptr::null_mut(),
                };
                let _ = libc::kevent(self.kq, &ev, 1, std::ptr::null_mut(), 0, std::ptr::null());
            }
        }
        Ok(())
    }
}

impl Drop for KqueueLoop {
    fn drop(&mut self) {
        // SAFETY: closing our own descriptor exactly once.
        unsafe { libc::close(self.kq) };
    }
}

impl EventLoop for KqueueLoop {
    fn register(&mut self, target: Target, token: Token, interest: Ready) -> io::Result<()> {
        let fd = match target {
            Target::Fd(fd) => fd,
            Target::Handle(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "handles are Windows-only",
                ));
            }
        };
        self.arm(fd, token, interest)
    }

    fn unregister(&mut self, target: Target) -> io::Result<()> {
        match target {
            Target::Fd(fd) => self.disarm_fd(fd),
            Target::Handle(_) => Ok(()),
        }
    }

    fn dispatch(
        &mut self,
        timeout: Option<Duration>,
        handler: &mut dyn EventHandler,
    ) -> io::Result<usize> {
        let ts = timeout.map(|d| libc::timespec {
            tv_sec: d.as_secs() as libc::time_t,
            tv_nsec: d.subsec_nanos() as libc::c_long,
        });
        let mut events = [blank_kevent(); 256];
        // SAFETY: eventlist points at our stack array; timeout optional.
        let n = unsafe {
            libc::kevent(
                self.kq,
                std::ptr::null(),
                0,
                events.as_mut_ptr(),
                events.len() as i32,
                match ts.as_ref() {
                    Some(t) => t as *const libc::timespec,
                    None => std::ptr::null(),
                },
            )
        };
        if n < 0 {
            let e = io::Error::last_os_error();
            if e.kind() == io::ErrorKind::Interrupted {
                return Ok(0);
            }
            return Err(e);
        }
        for ev in &events[..n as usize] {
            let mut ready = Ready::EMPTY;
            if ev.flags & (KQ_EV_ERROR | KQ_EV_EOF) != 0 {
                ready |= Ready::ERROR;
            }
            match ev.filter {
                KQ_EVFILT_READ => ready |= Ready::READ,
                KQ_EVFILT_WRITE => ready |= Ready::WRITE,
                _ => {}
            }
            handler.ready(ev.udata as usize as Token, ready);
        }
        Ok(n as usize)
    }
}

fn blank_kevent() -> libc::kevent {
    libc::kevent {
        ident: 0,
        filter: 0,
        flags: 0,
        fflags: 0,
        data: 0,
        udata: std::ptr::null_mut(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Recorder(Vec<(Token, Ready)>);
    impl EventHandler for Recorder {
        fn ready(&mut self, token: Token, ready: Ready) {
            self.0.push((token, ready));
        }
    }

    #[test]
    fn pipe_readiness_delivered() {
        let mut fds = [0 as libc::c_int; 2];
        // SAFETY: classic pipe(2) into a stack array.
        unsafe {
            assert_eq!(libc::pipe(fds.as_mut_ptr()), 0);
        }
        let (r, w) = (fds[0], fds[1]);

        let mut kq_loop = KqueueLoop::new().unwrap();
        kq_loop.register(Target::Fd(r), 777, Ready::READ).unwrap();

        // Nothing pending yet: short timeout yields nothing.
        let mut rec = Recorder(vec![]);
        kq_loop
            .dispatch(Some(Duration::from_millis(10)), &mut rec)
            .unwrap();
        assert!(rec.0.is_empty());

        // Write one byte → READ fires with our token.
        // SAFETY: w is our open write end; one-byte write of a literal.
        unsafe {
            assert_eq!(libc::write(w, b"x".as_ptr().cast(), 1), 1);
        }
        kq_loop
            .dispatch(Some(Duration::from_millis(100)), &mut rec)
            .unwrap();
        assert!(
            rec.0
                .iter()
                .any(|&(t, rdy)| t == 777 && rdy.contains(Ready::READ)),
            "expected READ on token 777, got {:?}",
            rec.0
        );

        // SAFETY: both ends owned by this test.
        unsafe {
            libc::close(r);
            libc::close(w);
        }
    }

    #[test]
    fn unregister_stops_delivery() {
        let mut fds = [0 as libc::c_int; 2];
        // SAFETY: pipe(2).
        unsafe {
            assert_eq!(libc::pipe(fds.as_mut_ptr()), 0);
        }
        let (r, w) = (fds[0], fds[1]);
        let mut kq_loop = KqueueLoop::new().unwrap();
        kq_loop.register(Target::Fd(r), 42, Ready::READ).unwrap();
        kq_loop.unregister(Target::Fd(r)).unwrap();

        // SAFETY: write then short wait.
        unsafe {
            libc::write(w, b"y".as_ptr().cast(), 1);
        }
        let mut rec = Recorder(vec![]);
        kq_loop
            .dispatch(Some(Duration::from_millis(20)), &mut rec)
            .unwrap();
        assert!(rec.0.is_empty());

        // SAFETY: owned descriptors.
        unsafe {
            libc::close(r);
            libc::close(w);
        }
    }
}
