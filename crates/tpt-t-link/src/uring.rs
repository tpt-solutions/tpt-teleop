//! io_uring-based transmit path (Linux only; spec §3.1 + §5.2).
//!
//! Datagrams are encoded **directly into submission-owned slots** — the
//! kernel reads what the serializer wrote, no intermediate copy between
//! "Serialize" and "Transmit" (the zero-copy claim of ARCHITECTURE §4).
//! Completion reaps free the slots; a bounded slot count makes the kernel
//! queue itself the backpressure signal (`stage` fails when all slots are
//! outstanding), which feeds the same congestion estimator as everything
//! else.
//!
//! Uses plain [`opcode::Send`] with an explicit destination address
//! (send(2)-with-addr semantics, kernels ≥ 5.6). Upgrading to
//! `opcode::SendZc` (true zero-copy, ≥ 5.19) is a drop-in change gated on a
//! runtime `Probe` check — deliberately deferred so v1 runs everywhere.

#![cfg(target_os = "linux")]

use std::io;
use std::net::SocketAddr;

use io_uring::{IoUring, opcode};

use crate::mux::MAX_DATAGRAM;

/// Default submission slots.
pub const DEFAULT_SLOTS: usize = 64;

/// Why a datagram could not be queued.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StageError {
    /// Every slot is staged or in flight — apply backpressure.
    Busy,
}

impl core::fmt::Display for StageError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            StageError::Busy => f.write_str("io_uring tx slots exhausted"),
        }
    }
}

impl std::error::Error for StageError {}

struct Slot {
    state: SlotState,
    buf: Box<[u8; MAX_DATAGRAM]>,
    len: usize,
    addr: libc::sockaddr_storage,
    addr_len: libc::socklen_t,
    next_free: u16, // freelist link while Free
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SlotState {
    Free,
    Staged,
    InFlight,
}

/// Bounded io_uring transmit queue over one (optionally unconnected) UDP fd.
pub struct UringTx {
    ring: IoUring,
    fd: i32,
    slots: Box<[Slot]>,
    free_head: u16, // NONE = sentinel for "no free slot"
    staged: usize,
    inflight: usize,
}

const NONE: u16 = u16::MAX;

impl UringTx {
    /// Creates a transmit ring bound to `fd` with `slots` submission slots.
    /// Fails when the kernel lacks io_uring support (containers often do);
    /// callers fall back to the portable [`crate::mux::UdpMux`] send path.
    pub fn new(fd: i32, slots: usize) -> io::Result<Self> {
        let slots = slots.max(1);
        let ring = IoUring::new(slots as u32)?;
        // Freelist: slot i links to i+1; the tail links NONE; head is 0.
        let mut slot_vec: Vec<Slot> = Vec::with_capacity(slots);
        for i in 0..slots {
            let next_free = if i + 1 < slots { (i + 1) as u16 } else { NONE };
            slot_vec.push(Slot {
                state: SlotState::Free,
                buf: Box::new([0u8; MAX_DATAGRAM]),
                len: 0,
                // SAFETY: sockaddr_storage is plain-old-data; an all-zero
                // value is the documented "unspecified address" state and is
                // overwritten by fill_sockaddr before any kernel read.
                addr: unsafe { std::mem::zeroed() },
                addr_len: 0,
                next_free,
            });
        }
        Ok(Self {
            ring,
            fd,
            slots: slot_vec.into_boxed_slice(),
            free_head: 0,
            staged: 0,
            inflight: 0,
        })
    }

    /// Slots not currently staged or in flight.
    #[inline]
    pub fn available(&self) -> usize {
        self.slots.len() - self.staged - self.inflight
    }

    /// Slots waiting for [`submit_staged`](Self::submit_staged).
    #[inline]
    pub fn staged(&self) -> usize {
        self.staged
    }

    /// Slots submitted to the kernel awaiting completion.
    #[inline]
    pub fn inflight(&self) -> usize {
        self.inflight
    }

    /// Claims a free slot and lets `fill` encode the datagram **into it**
    /// (returning the byte count). The destination is captured alongside so
    /// the SQE carries a sendto(2)-style address.
    pub fn stage<F>(&mut self, peer: SocketAddr, fill: F) -> Result<(), StageError>
    where
        F: FnOnce(&mut [u8]) -> usize,
    {
        if self.free_head == NONE {
            return Err(StageError::Busy);
        }
        let idx = self.free_head;
        let slot = &mut self.slots[idx as usize];
        self.free_head = slot.next_free;

        slot.len = fill(&mut slot.buf[..]);
        fill_sockaddr(&mut slot.addr, &mut slot.addr_len, peer);
        slot.state = SlotState::Staged;
        self.staged += 1;
        Ok(())
    }

    /// Pushes one SQE per staged slot and submits to the kernel. Returns how
    /// many submissions were accepted (SQ-full leaves the rest for retry).
    pub fn submit_staged(&mut self) -> io::Result<usize> {
        let mut pushed = 0usize;
        for idx in 0..self.slots.len() {
            if self.slots[idx].state != SlotState::Staged {
                continue;
            }
            let slot = &self.slots[idx];
            let entry = opcode::Send::new(
                io_uring::types::Fd(self.fd), // raw fd, not a registered file
                slot.buf.as_ptr(),
                slot.len as u32,
            )
            .dest_addr(&slot.addr as *const libc::sockaddr_storage as *const libc::sockaddr)
            .dest_addr_len(slot.addr_len)
            .build()
            .user_data(idx as u64);

            // SAFETY: the entry references slot memory owned by this struct,
            // which outlives the in-flight window (slots are recycled only on
            // completion); the submission queue is single-consumer by design.
            unsafe {
                if self.ring.submission().push(&entry).is_err() {
                    break; // SQ full — remaining slots retry on next call
                }
            }
            self.slots[idx].state = SlotState::InFlight;
            pushed += 1;
        }
        self.staged -= pushed;
        self.inflight += pushed;
        if pushed > 0 {
            self.ring.submit()?;
        }
        Ok(pushed)
    }

    /// Blocks until at least `n` completions land (also flushes submissions).
    pub fn wait(&mut self, n: usize) -> io::Result<()> {
        self.ring.submit_and_wait(n).map(|_| ())
    }

    /// Reaps finished completions (non-blocking). `on_error` receives raw
    /// negative errno values from failed sends. Returns the reaped count.
    pub fn reap(&mut self, mut on_error: impl FnMut(i32)) -> usize {
        let mut reaped = 0usize;
        // SAFETY: draining our own completion queue; the iterator handles CQ
        // synchronization over its lifetime.
        for cqe in self.ring.completion() {
            let idx = cqe.user_data() as usize;
            reaped += 1;
            self.inflight = self.inflight.saturating_sub(1);
            if cqe.result() < 0 {
                on_error(cqe.result());
            }
            if idx < self.slots.len() {
                let slot = &mut self.slots[idx];
                slot.state = SlotState::Free;
                slot.next_free = self.free_head;
                self.free_head = idx as u16;
            }
        }
        reaped
    }
}

/// Writes `peer` into a zeroed `sockaddr_storage`.
fn fill_sockaddr(ss: &mut libc::sockaddr_storage, len: &mut libc::socklen_t, peer: SocketAddr) {
    // SAFETY: zeroing within sockaddr_storage bounds before any read.
    unsafe {
        core::ptr::write_bytes(
            ss as *mut libc::sockaddr_storage as *mut u8,
            0,
            size_of::<libc::sockaddr_storage>(),
        );
    }
    match peer.ip() {
        std::net::IpAddr::V4(v4) => {
            // SAFETY: AF_INET sockaddr_in is smaller than sockaddr_storage.
            let sin =
                unsafe { &mut *(ss as *mut libc::sockaddr_storage as *mut libc::sockaddr_in) };
            sin.sin_family = libc::AF_INET as libc::sa_family_t;
            sin.sin_port = peer.port().to_be();
            sin.sin_addr.s_addr = u32::from_be_bytes(v4.octets());
            *len = size_of::<libc::sockaddr_in>() as libc::socklen_t;
        }
        std::net::IpAddr::V6(v6) => {
            // SAFETY: AF_INET6 sockaddr_in6 fits inside sockaddr_storage.
            let sin6 =
                unsafe { &mut *(ss as *mut libc::sockaddr_storage as *mut libc::sockaddr_in6) };
            sin6.sin6_family = libc::AF_INET6 as libc::sa_family_t;
            sin6.sin6_port = peer.port().to_be();
            sin6.sin6_addr.s6_addr = v6.octets();
            *len = size_of::<libc::sockaddr_in6>() as libc::socklen_t;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::UdpSocket;

    fn loopback_pair() -> io::Result<(UdpSocket, SocketAddr)> {
        let rx = UdpSocket::bind(("127.0.0.1", 0))?;
        let tx = UdpSocket::bind(("127.0.0.1", 0))?;
        Ok((rx, tx.local_addr()?))
    }

    /// Builds a ring for `sock`, or `None` where io_uring is unavailable
    /// (old kernels, seccomp-hardened CI containers).
    fn try_ring(sock: &UdpSocket, slots: usize) -> Option<UringTx> {
        use std::os::fd::AsRawFd;
        match UringTx::new(sock.as_raw_fd(), slots) {
            Ok(r) => Some(r),
            Err(e)
                if e.kind() == io::ErrorKind::Unsupported
                    || e.raw_os_error() == Some(libc::ENOSYS)
                    || e.raw_os_error() == Some(libc::EPERM) =>
            {
                eprintln!("skipping io_uring test: {e}");
                None
            }
            Err(e) => panic!("io_uring setup failed: {e}"),
        }
    }

    #[test]
    fn uring_loopback_roundtrip_and_slot_recycling() {
        let (rx, peer_a) = loopback_pair().expect("loopback UDP");
        let (tx_sock, peer_b) = loopback_pair().expect("loopback UDP");
        let Some(mut ring) = try_ring(&tx_sock, 4) else {
            return;
        };

        // Stage two datagrams written straight into submission slots.
        ring.stage(peer_a, |buf| {
            buf[..5].copy_from_slice(b"hello");
            5
        })
        .unwrap();
        ring.stage(peer_b, |buf| {
            buf[..5].copy_from_slice(b"world");
            5
        })
        .unwrap();
        assert_eq!(ring.available(), 2);

        ring.submit_staged().unwrap();
        ring.wait(2).unwrap();
        let mut errors = Vec::new();
        assert_eq!(ring.reap(|e| errors.push(e)), 2);
        assert!(errors.is_empty());
        assert_eq!(ring.inflight(), 0);
        assert_eq!(ring.available(), 4);

        // Both receivers got their datagram with intact payloads.
        rx.set_read_timeout(Some(std::time::Duration::from_secs(2)))
            .unwrap();
        let mut buf = [0u8; 64];
        let (n, _) = rx.recv_from(&mut buf).expect("datagram A");
        assert_eq!(&buf[..n], b"hello");
        tx_sock
            .set_read_timeout(Some(std::time::Duration::from_secs(2)))
            .unwrap();
        let (n, _) = tx_sock.recv_from(&mut buf).expect("datagram B");
        assert_eq!(&buf[..n], b"world");
    }

    #[test]
    fn stage_fails_when_slots_exhausted() {
        let (_rx, peer) = match loopback_pair() {
            Ok(v) => v,
            Err(e) => panic!("no loopback UDP: {e}"),
        };
        let tx_sock = UdpSocket::bind(("127.0.0.1", 0)).unwrap();
        let Some(mut ring) = try_ring(&tx_sock, 2) else {
            return;
        };
        ring.stage(peer, |_| 1).unwrap();
        ring.stage(peer, |_| 1).unwrap();
        assert_eq!(ring.stage(peer, |_| 1), Err(StageError::Busy));
    }

    #[test]
    fn sockaddr_builder_covers_v4_and_v6() {
        let mut ss: libc::sockaddr_storage = unsafe {
            // SAFETY: POD type; zeroed == INADDR_ANY/unspecified, valid to
            // overwrite field-by-field in fill_sockaddr below.
            std::mem::zeroed()
        };
        let mut len: libc::socklen_t = 0;
        let v4: SocketAddr = "127.0.0.1:8080".parse().unwrap();
        fill_sockaddr(&mut ss, &mut len, v4);
        assert_eq!(len as usize, size_of::<libc::sockaddr_in>());
        // SAFETY: family was set to AF_INET above, so sockaddr_in is the
        // matching interpretation of the storage.
        let sin = unsafe { &*(std::ptr::addr_of!(ss) as *const libc::sockaddr_in) };
        assert_eq!(sin.sin_family as i32, libc::AF_INET);
        assert_eq!(sin.sin_port, 8080u16.to_be());
        assert_eq!(sin.sin_addr.s_addr.to_be_bytes(), [127, 0, 0, 1]);

        let v6: SocketAddr = "[::1]:9090".parse().unwrap();
        fill_sockaddr(&mut ss, &mut len, v6);
        assert_eq!(len as usize, size_of::<libc::sockaddr_in6>());
        // SAFETY: family was set to AF_INET6 above.
        let sin6 = unsafe { &*(std::ptr::addr_of!(ss) as *const libc::sockaddr_in6) };
        assert_eq!(sin6.sin6_family as i32, libc::AF_INET6);
        assert_eq!(sin6.sin6_port, 9090u16.to_be());
        assert!(sin6.sin6_addr.s6_addr.iter().take(15).all(|&b| b == 0));
        assert_eq!(sin6.sin6_addr.s6_addr[15], 1);
    }
}
