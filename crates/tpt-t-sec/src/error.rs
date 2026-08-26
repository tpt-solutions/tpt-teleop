//! Security error types (Phase 11, spec §5.5).

use core::fmt;

/// Errors surfaced by the security subsystem.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecError {
    /// Symmetric key had the wrong length for the chosen AEAD.
    InvalidKeyLength,
    /// A primitive AEAD / signature / agreement operation failed
    /// (`ring::error::Unspecified` — opaque by policy).
    Crypto,
    /// Key generation could not be seeded.
    KeyGen,
    /// An attestation signature did not verify, or the signing key is not in
    /// the trusted set (zero-trust reject).
    AttestationFailed,
    /// The handshake reached an inconsistent state (e.g. mismatched peer).
    Handshake(&'static str),
    /// The caller's destination buffer was too small for the operation.
    BufferTooSmall,
    /// The target ring was full when a decrypted block was ready to push.
    RingFull,
    /// (De)serialization of a handshake message failed.
    Serialize,
    /// The trust store has no anchor for the claimed unit.
    UnknownUnit,
}

impl fmt::Display for SecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SecError::InvalidKeyLength => f.write_str("invalid key length"),
            SecError::Crypto => f.write_str("cryptographic operation failed"),
            SecError::KeyGen => f.write_str("key generation failed"),
            SecError::AttestationFailed => f.write_str("attestation failed"),
            SecError::Handshake(s) => write!(f, "handshake failed: {s}"),
            SecError::BufferTooSmall => f.write_str("buffer too small"),
            SecError::RingFull => f.write_str("ring full"),
            SecError::Serialize => f.write_str("serialize/deserialize failed"),
            SecError::UnknownUnit => f.write_str("unknown unit"),
        }
    }
}

impl std::error::Error for SecError {}

impl From<ring::error::Unspecified> for SecError {
    fn from(_: ring::error::Unspecified) -> Self {
        SecError::Crypto
    }
}

impl From<ring::error::KeyRejected> for SecError {
    fn from(_: ring::error::KeyRejected) -> Self {
        SecError::InvalidKeyLength
    }
}
