//! Zero-trust security model, RBAC, and end-to-end encryption for
//! tpt-teleop (spec §5.5 / §5.6).
//!
//! The crate is composed of small, auditable layers:
//!
//! * [`cipher`] — AEAD wrappers over `ring` (AES-256-GCM, ChaCha20-Poly1305)
//!   with zero-copy seal/open paths that decrypt straight into a ring slot.
//! * [`identity`] — per-unit Ed25519 signing identity and signed
//!   [`Attestation`] (binds a session to a known key).
//! * [`session`] — mutually-authenticated, forward-secret key agreement
//!   (ephemeral X25519 ECDH + HKDF) producing a [`SecureSession`].
//! * [`rbac`] — roles, permissions, and the per-action [`Policy`].
//! * [`zerotrust`] — the always-verify broker: attestation → authenticated
//!   [`Principal`](crate::rbac::Principal) → authorized action.
//! * [`link`] — `tpt-t-link` integration: a [`SecureMux`] speaking E2EE over
//!   `Channel::Secure`, decrypting in place into an [`SpscRing`].
//! * [`cloud`] — `tpt-t-cloud` integration: the fleet-dispatch authorization
//!   gate ([`FleetAuthz`]) built from attestation + RBAC.
//!
//! # Zero-trust posture
//!
//! Nothing is trusted by network position. Every peer must present a valid
//! attestation whose signing key is enrolled in a [`TrustStore`], and every
//! privileged action is re-checked against RBAC. Session keys are
//! single-use-ephemeral, so a later key compromise cannot decrypt past
//! traffic.

pub mod cipher;
pub mod cloud;
pub mod identity;
pub mod link;
pub mod rbac;
pub mod session;
pub mod zerotrust;

pub use cipher::{
    CipherSuite, CryptoBox, MAX_SECURE_BLOCK, NONCE_LEN, SecureBlock, TAG_LEN, decrypt_into_ring,
};
pub use cloud::FleetAuthz;
pub use error::SecError;
pub use identity::{Attestation, DeviceIdentity};
pub use link::SecureMux;
pub use rbac::{Permission, Policy, Principal, Role, authorize_dispatch, dispatch_permission};
pub use session::{
    HandshakeInit, HandshakeResp, PendingHandshake, SecureSession, begin_handshake,
    finish_handshake, respond_handshake,
};
pub use zerotrust::{TrustStore, ZeroTrustBroker};

/// Crate version (from Cargo metadata).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub mod error;
