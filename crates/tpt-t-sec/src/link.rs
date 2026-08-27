//! Integration shim between `tpt-t-sec` and `tpt-t-link` (spec §5.5).
//!
//! `SecureMux` wraps a [`UdpMux`] and a negotiated [`SecureSession`]: control
//! and telemetry commands are rkyv-serialized, AEAD-sealed, and transmitted
//! on the link's `Channel::Secure`. On receive, sealed envelopes are opened
//! **in place** and the recovered plaintext is pushed straight into an
//! [`SpscRing`] slot (the spec §5.5 zero-copy decrypt path) for the consumer
//! thread to deserialize — no heap allocation on the hot path.

use std::io;
use std::net::SocketAddr;

use tpt_t_core::ser::{AlignedBuf, ControlCommand, TelemetryPacket, serialize_into};
use tpt_t_link::mux::{Inbound, MAX_DATAGRAM, RxBuffer, UdpMux, secure_inner};
use tpt_t_ring::SpscRing;

use crate::cipher::{CryptoBox, SecureBlock, decrypt_into_ring};
use crate::error::SecError;
use crate::session::SecureSession;

/// AAD domain tags separating sealed channels from one another.
pub const AAD_CONTROL: &[u8] = b"tpt-sec-control";
pub const AAD_TELEMETRY: &[u8] = b"tpt-sec-telemetry";

/// A `UdpMux` bound to one port, speaking only E2EE over `Channel::Secure`
/// using a pre-negotiated session.
pub struct SecureMux {
    mux: UdpMux,
    session: SecureSession,
}

impl SecureMux {
    /// Binds a secure mux on `port` (use `0` for an ephemeral test port).
    pub fn bind(port: u16, session: SecureSession) -> io::Result<Self> {
        Ok(Self {
            mux: UdpMux::bind(port)?,
            session,
        })
    }

    /// Local bound address.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.mux.local_addr()
    }

    /// The negotiated session (peer id / role / suite).
    pub fn session(&self) -> &SecureSession {
        &self.session
    }

    /// Seals a control command and transmits it to `dst`. Returns bytes sent.
    pub fn send_secure_control(
        &mut self,
        cmd: &ControlCommand,
        dst: SocketAddr,
        now_ns: u64,
    ) -> io::Result<usize> {
        let mut scratch = AlignedBuf::new();
        let n = serialize_into(cmd, &mut scratch).map_err(io::Error::other)?;
        self.send_sealed(&scratch[..n], AAD_CONTROL, secure_inner::CONTROL, dst, now_ns)
    }

    /// Seals a telemetry packet and transmits it to `dst`.
    pub fn send_secure_telemetry(
        &mut self,
        pkt: &TelemetryPacket,
        dst: SocketAddr,
        now_ns: u64,
    ) -> io::Result<usize> {
        let mut scratch = AlignedBuf::new();
        let n = serialize_into(pkt, &mut scratch).map_err(io::Error::other)?;
        self.send_sealed(&scratch[..n], AAD_TELEMETRY, secure_inner::TELEMETRY, dst, now_ns)
    }

    /// Seals arbitrary plaintext and transmits it on the secure channel.
    pub fn send_secure_raw(
        &mut self,
        plaintext: &[u8],
        dst: SocketAddr,
        now_ns: u64,
    ) -> io::Result<usize> {
        self.send_sealed(plaintext, AAD_CONTROL, secure_inner::CONTROL, dst, now_ns)
    }

    fn send_sealed(
        &mut self,
        plaintext: &[u8],
        aad: &[u8],
        inner: u8,
        dst: SocketAddr,
        now_ns: u64,
    ) -> io::Result<usize> {
        let sealed = self
            .session
            .seal(plaintext, aad)
            .map_err(io::Error::other)?;
        let mut buf = [0u8; MAX_DATAGRAM];
        let n = self.mux.write_secure_frame(&sealed, inner, &mut buf)?;
        self.mux.send_framed(dst, &buf[..n], now_ns).map(|_| n)
    }

    /// Receives one datagram. If it is a `Channel::Secure` envelope, it is
    /// decrypted in place and the plaintext is pushed onto `ring`; returns the
    /// number of blocks pushed (0 or 1). Non-secure frames and rejects are
    /// skipped so the caller can keep a tight receive loop.
    ///
    /// The AEAD AAD domain is selected from the frame's inner-channel tag so a
    /// telemetry envelope (sealed under `AAD_TELEMETRY`) is opened with the
    /// matching AAD — a control-only AAD previously made telemetry decrypt
    /// silently fail.
    pub fn recv_decrypt<const N: usize>(
        &mut self,
        rx: &mut RxBuffer,
        ring: &SpscRing<SecureBlock<N>>,
    ) -> io::Result<usize> {
        match self.mux.recv_frame(rx)? {
            Some(Ok(Inbound::Secure { sealed, inner, .. })) => {
                let aad = if inner == secure_inner::TELEMETRY {
                    AAD_TELEMETRY
                } else {
                    AAD_CONTROL
                };
                let pushed = decrypt_into_ring(self.session.crypto(), sealed, aad, ring).is_ok();
                Ok(if pushed { 1 } else { 0 })
            }
            _ => Ok(0),
        }
    }
}

/// Helper: open a sealed envelope from an `Event::Secure` delivered by
/// `NetService` directly into a ring slot (no intermediate allocation).
/// `inner` is the [`secure_inner`](tpt_t_link::mux::secure_inner) channel tag
/// selecting the AEAD AAD domain. Returns the plaintext length on success.
pub fn decrypt_event_into_ring<const N: usize>(
    box_: &CryptoBox,
    sealed: &[u8],
    inner: u8,
    ring: &SpscRing<SecureBlock<N>>,
) -> Result<usize, SecError> {
    let aad = if inner == secure_inner::TELEMETRY {
        AAD_TELEMETRY
    } else {
        AAD_CONTROL
    };
    decrypt_into_ring(box_, sealed, aad, ring)
}
