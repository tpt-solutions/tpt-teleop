//! Device identity and signed attestation (spec §5.5 zero-trust).
//!
//! Every unit holds a long-lived Ed25519 signing key. When it opens a secure
//! session it generates a fresh X25519 **ephemeral** agreement key and signs
//! `(unit_id ‖ role ‖ ephemeral_public)` with its signing key. The peer
//! verifies the signature against the unit's registered public key (the trust
//! anchor) before deriving any shared secret — this is the zero-trust "never
//! trust, always verify" check that binds a session to a known identity.

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use getrandom::getrandom;

use crate::error::SecError;
use crate::rbac::Role;

/// Ed25519 public key length.
pub const SIGNING_PUB_LEN: usize = 32;
/// X25519 public key length.
pub const AGREEMENT_PUB_LEN: usize = 32;
/// Ed25519 signature length.
pub const SIGNATURE_LEN: usize = 64;

fn role_byte(role: Role) -> u8 {
    match role {
        Role::Guest => 0,
        Role::Observer => 1,
        Role::Operator => 2,
        Role::Admin => 3,
        Role::AiAgent => 4,
    }
}

/// Inverse of [`role_byte`]; unknown values map to [`Role::Guest`].
fn role_from_byte(b: u8) -> Role {
    match b {
        0 => Role::Guest,
        1 => Role::Observer,
        2 => Role::Operator,
        3 => Role::Admin,
        4 => Role::AiAgent,
        _ => Role::Guest,
    }
}

/// Wire length of an [`Attestation`] serialized by [`Attestation::to_bytes`].
pub const ATTESTATION_WIRE_LEN: usize = 8 + 1 + SIGNING_PUB_LEN + AGREEMENT_PUB_LEN + SIGNATURE_LEN;

/// The signed identity claim a peer presents during the handshake.
///
/// `sig` is `Ed25519( unit_id:u64le ‖ role:u8 ‖ eph_pub:32 )`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub struct Attestation {
    /// Claimed unit id.
    pub unit_id: u64,
    /// Claimed role.
    pub role: Role,
    /// Signing public key (the identity being attested).
    pub signing_pub: [u8; SIGNING_PUB_LEN],
    /// Ephemeral X25519 public key used for this session's ECDH.
    pub eph_pub: [u8; AGREEMENT_PUB_LEN],
    /// Ed25519 signature over the signed fields.
    pub sig: [u8; SIGNATURE_LEN],
}

impl Attestation {
    /// The canonical byte string an attestation signs.
    #[inline]
    fn signed_message(unit_id: u64, role: Role, eph_pub: &[u8; AGREEMENT_PUB_LEN]) -> [u8; 41] {
        let mut m = [0u8; 41];
        m[..8].copy_from_slice(&unit_id.to_le_bytes());
        m[8] = role_byte(role);
        m[9..].copy_from_slice(eph_pub);
        m
    }

    /// Builds an attestation by signing with `identity`'s key.
    pub fn sign(
        identity: &DeviceIdentity,
        eph_pub: &[u8; AGREEMENT_PUB_LEN],
    ) -> Result<Self, SecError> {
        let msg = Self::signed_message(identity.unit_id, identity.role, eph_pub);
        let sig: [u8; SIGNATURE_LEN] = identity.signing.sign(&msg).to_bytes();
        Ok(Self {
            unit_id: identity.unit_id,
            role: identity.role,
            signing_pub: identity.signing_pub,
            eph_pub: *eph_pub,
            sig,
        })
    }

    /// Verifies the signature against the embedded signing public key.
    pub fn verify(&self) -> bool {
        let msg = Self::signed_message(self.unit_id, self.role, &self.eph_pub);
        let Ok(peer_key) = VerifyingKey::from_bytes(&self.signing_pub) else {
            return false;
        };
        let sig = Signature::from_bytes(&self.sig);
        peer_key.verify(&msg, &sig).is_ok()
    }

    /// Serializes the attestation to a fixed-layout byte string for transport
    /// (e.g. an HTTP `Authorization` header or a handshake message).
    pub fn to_bytes(&self) -> [u8; ATTESTATION_WIRE_LEN] {
        let mut b = [0u8; ATTESTATION_WIRE_LEN];
        b[0..8].copy_from_slice(&self.unit_id.to_le_bytes());
        b[8] = role_byte(self.role);
        b[9..9 + SIGNING_PUB_LEN].copy_from_slice(&self.signing_pub);
        let o = 9 + SIGNING_PUB_LEN;
        b[o..o + AGREEMENT_PUB_LEN].copy_from_slice(&self.eph_pub);
        let o = o + AGREEMENT_PUB_LEN;
        b[o..o + SIGNATURE_LEN].copy_from_slice(&self.sig);
        b
    }

    /// Reverses [`to_bytes`](Self::to_bytes). The signature is *not* verified
    /// here — callers must [`verify`](Self::verify) the result (the zero-trust
    /// broker does this on every admission).
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, SecError> {
        if bytes.len() < ATTESTATION_WIRE_LEN {
            return Err(SecError::Serialize);
        }
        let unit_id = u64::from_le_bytes(bytes[0..8].try_into().unwrap());
        let role = role_from_byte(bytes[8]);
        let mut signing_pub = [0u8; SIGNING_PUB_LEN];
        signing_pub.copy_from_slice(&bytes[9..9 + SIGNING_PUB_LEN]);
        let o = 9 + SIGNING_PUB_LEN;
        let mut eph_pub = [0u8; AGREEMENT_PUB_LEN];
        eph_pub.copy_from_slice(&bytes[o..o + AGREEMENT_PUB_LEN]);
        let o = o + AGREEMENT_PUB_LEN;
        let mut sig = [0u8; SIGNATURE_LEN];
        sig.copy_from_slice(&bytes[o..o + SIGNATURE_LEN]);
        Ok(Self {
            unit_id,
            role,
            signing_pub,
            eph_pub,
            sig,
        })
    }
}

/// A unit's long-lived identity: an Ed25519 signing key plus its declared
/// unit id and role. Agreement keys are ephemeral per handshake, so they are
/// not stored here.
pub struct DeviceIdentity {
    unit_id: u64,
    role: Role,
    signing: SigningKey,
    signing_pub: [u8; SIGNING_PUB_LEN],
}

impl std::fmt::Debug for DeviceIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceIdentity")
            .field("unit_id", &self.unit_id)
            .field("role", &self.role)
            .field("signing_pub", &self.signing_pub)
            .finish()
    }
}

impl DeviceIdentity {
    /// Generates a fresh identity with the given unit id and role.
    pub fn generate(unit_id: u64, role: Role) -> Result<Self, SecError> {
        let mut seed = [0u8; 32];
        getrandom(&mut seed).map_err(|_| SecError::KeyGen)?;
        let signing = SigningKey::from(seed);
        let pub_doc = signing.verifying_key();
        let mut signing_pub = [0u8; SIGNING_PUB_LEN];
        signing_pub.copy_from_slice(pub_doc.as_bytes());
        Ok(Self {
            unit_id,
            role,
            signing,
            signing_pub,
        })
    }

    /// Unit id.
    pub fn unit_id(&self) -> u64 {
        self.unit_id
    }

    /// Role.
    pub fn role(&self) -> Role {
        self.role
    }

    /// The signing public key bytes (also embedded in attestations).
    pub fn public_key(&self) -> &[u8; SIGNING_PUB_LEN] {
        &self.signing_pub
    }

    /// Signs an arbitrary message (returns 64 raw signature bytes).
    pub fn sign_message(&self, msg: &[u8]) -> [u8; SIGNATURE_LEN] {
        self.signing.sign(msg).to_bytes()
    }

    /// Verifies a signature made by this identity's public key.
    pub fn verify_message(&self, msg: &[u8], sig: &[u8; SIGNATURE_LEN]) -> bool {
        let Ok(key) = VerifyingKey::from_bytes(&self.signing_pub) else {
            return false;
        };
        let sig = Signature::from_bytes(sig);
        key.verify(msg, &sig).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_signs_and_verifies() {
        let id = DeviceIdentity::generate(42, Role::Operator).unwrap();
        let msg = b"control intent";
        let sig = id.sign_message(msg);
        assert!(id.verify_message(msg, &sig));
        assert!(!id.verify_message(b"tampered", &sig));
    }

    #[test]
    fn attestation_roundtrip() {
        let id = DeviceIdentity::generate(7, Role::Admin).unwrap();
        let eph = [0xABu8; AGREEMENT_PUB_LEN];
        let att = Attestation::sign(&id, &eph).unwrap();
        assert_eq!(att.unit_id, 7);
        assert_eq!(att.role, Role::Admin);
        assert!(att.verify());
        // A different key's attestation must not verify against id's pub.
        let other = DeviceIdentity::generate(8, Role::Observer).unwrap();
        let mut forged = att;
        forged.signing_pub = *other.public_key();
        // Signature no longer matches the new pub key.
        assert!(!forged.verify());
    }

    #[test]
    fn attestation_wire_format_roundtrips() {
        let id = DeviceIdentity::generate(11, Role::AiAgent).unwrap();
        let eph = [0x55u8; AGREEMENT_PUB_LEN];
        let att = Attestation::sign(&id, &eph).unwrap();
        let wire = att.to_bytes();
        assert_eq!(wire.len(), ATTESTATION_WIRE_LEN);
        let back = Attestation::from_bytes(&wire).unwrap();
        assert_eq!(back, att);
        assert!(back.verify());
        // Truncated wire is rejected.
        assert!(Attestation::from_bytes(&wire[..8]).is_err());
    }
}
