//! Linux backend: custom epoll readiness loop (spec §3.1 permits "write a
//! custom epoll event loop from scratch"). io_uring-based zero-copy network
//! transmit is layered at the link crate (Phase 7); readiness multiplexing
//! here stays dependency-free via raw epoll syscalls through `libc`.

use std::collections::HashMap;
use std::io;
use std::time::Duration;

use super::{EventHandler, EventLoop, Ready, Target, Token};

const EPOLL_CTL_ADD: i32 = 1;
const EPOLL_CTL_DEL: i32 = 2;
const EPOLLIN: u32 = 0x001;
const EPOLLOUT: u32 = 0x004;
const EPOLLERR: u32 = 0x008;
const EPOLLHUP: u32 = 0x010;
const EPOLLET: u32 = 1 << 31;

fn to_epoll_mask(interest: Ready) -> u32 {
    let mut m = EPOLLERR | EPOLLHUP | EPOLLET; // always-on per epoll semantics
    if interest.intersects(Ready::READ) {
        m |= EPOLLIN;
    }
    if interest.intersects(Ready::WRITE) {
        m |= EPOLLOUT;
    }
    m
}

/// epoll-backed edge-triggered readiness loop.
#[derive(Debug)]
pub struct EpollLoop {
    epfd: i32,
    regs: HashMap<Token, (i32, u32)>, // token -> (fd, mask)
}

// SAFETY: unique epfd ownership; registrations are plain bookkeeping.
unsafe impl Send for EpollLoop {}
// SAFETY: &EpollLoop only forwards stored fds/epfd into syscalls by value.
unsafe impl Sync for EpollLoop {}

impl EpollLoop {
    /// Creates the epoll instance.
    pub fn new() -> io::Result<Self> {
        // SAFETY: epoll_create1(0) has no pointer arguments.
        let epfd = unsafe { libc::epoll_create1(0) };
        if epfd < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            epfd,
            regs: HashMap::new(),
        })
    }
}

impl Drop for EpollLoop {
    fn drop(&mut self) {
        // SAFETY: closing our own descriptor exactly once.
        unsafe { libc::close(self.epfd) };
    }
}

impl EventLoop for EpollLoop {
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
        let mut ev = libc::epoll_event {
            events: to_epoll_mask(interest),
            u64: token,
        };
        // SAFETY: valid epfd/fd and stack-resident epoll_event.
        if unsafe { libc::epoll_ctl(self.epfd, EPOLL_CTL_ADD, fd, &mut ev) } < 0 {
            return Err(io::Error::last_os_error());
        }
        self.regs.insert(token, (fd, ev.events));
        Ok(())
    }

    fn unregister(&mut self, target: Target) -> io::Result<()> {
        let fd = match target {
            Target::Fd(fd) => fd,
            Target::Handle(_) => return Ok(()),
        };
        let Some(token) = self
            .regs
            .iter()
            .find(|(_, (f, _))| *f == fd)
            .map(|(t, _)| *t)
        else {
            return Ok(());
        };
        self.regs.remove(&token);
        // SAFETY: DEL on our own epfd for a previously registered fd; the
        // event argument is ignored by the kernel but must be non-null on
        // old kernels, hence the zeroed dummy.
        let mut ev: libc::epoll_event = unsafe { std::mem::zeroed() };
        // SAFETY: deregistering a previously registered fd from our own epfd.
        if unsafe { libc::epoll_ctl(self.epfd, EPOLL_CTL_DEL, fd, &mut ev) } < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    fn dispatch(
        &mut self,
        timeout: Option<Duration>,
        handler: &mut dyn EventHandler,
    ) -> io::Result<usize> {
        let ms = timeout
            .map(|d| d.as_millis().min(i32::MAX as u128) as i32)
            .unwrap_or(-1);
        let mut events = [libc::epoll_event { events: 0, u64: 0 }; 256];
        // SAFETY: events is our stack array sized explicitly below.
        let n =
            unsafe { libc::epoll_wait(self.epfd, events.as_mut_ptr(), events.len() as i32, ms) };
        if n < 0 {
            let e = io::Error::last_os_error();
            if e.kind() == io::ErrorKind::Interrupted {
                return Ok(0);
            }
            return Err(e);
        }
        for ev in &events[..n as usize] {
            let token = ev.u64 as Token;
            let mut ready = Ready::EMPTY;
            if ev.events & EPOLLIN != 0 {
                ready |= Ready::READ;
            }
            if ev.events & EPOLLOUT != 0 {
                ready |= Ready::WRITE;
            }
            if ev.events & (EPOLLERR | EPOLLHUP) != 0 {
                ready |= Ready::ERROR;
            }
            handler.ready(token, ready);
        }
        Ok(n as usize)
    }
}
