//! Zero-trust session establishment (spec §5.5): a mutually-authenticated,
//! forward-secret key agreement.
//!
//! ```text
//! initiator ── Attestation(I, ephA) ───────────────────────▶ responder
//! initiator ◀─────────────────────── Attestation(R, ephB) ── responder
//! ```
//!
//! Each attestation signs `(unit_id ‖ role ‖ ephemeral_pub)` with the
//! sender's long-lived Ed25519 key. Both sides verify the peer attestation,
//! then derive a shared secret via ephemeral-ephemeral X25519 ECDH and expand
//! it with HKDF-SHA256 into a [`CryptoBox`] key. Because the agreement keys
//! are single-use, a later key compromise does not decrypt past traffic
//! (perfect forward secrecy). Because the ephemeral keys are signed, an
//! attacker cannot mount a man-in-the-middle without a trusted signing key.

use ring::agreement::{self, EphemeralPrivateKey, UnparsedPublicKey, X25519};
use ring::hkdf;
use ring::rand::SystemRandom;

use crate::cipher::{CipherSuite, CryptoBox};
use crate::error::SecError;
use crate::identity::{Attestation, DeviceIdentity};
use crate::rbac::Role;

/// First handshake message (initiator → responder).
#[derive(Debug, Clone)]
pub struct HandshakeInit {
    /// Initiator's signed identity + ephemeral agreement key.
    pub attestation: Attestation,
    /// Cipher suites the initiator supports, most-preferred first.
    pub suites: Vec<CipherSuite>,
}

/// Second handshake message (responder → initiator).
#[derive(Debug, Clone)]
pub struct HandshakeResp {
    /// Responder's signed identity + ephemeral agreement key.
    pub attestation: Attestation,
    /// Suite the responder selected from the intersection.
    pub suite: CipherSuite,
}

/// Opaque initiator-side handshake state holding the ephemeral agreement key
/// (consumed when the responder's reply arrives).
pub struct PendingHandshake {
    eph: EphemeralPrivateKey,
}

/// A mutually-authenticated, encrypted session between two units.
pub struct SecureSession {
    peer_id: u64,
    peer_role: Role,
    crypto: CryptoBox,
    suite: CipherSuite,
}

impl std::fmt::Debug for SecureSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SecureSession")
            .field("peer_id", &self.peer_id)
            .field("peer_role", &self.peer_role)
            .field("suite", &self.suite)
            .finish()
    }
}

impl SecureSession {
    /// The authenticated peer unit id.
    #[inline]
    pub fn peer_id(&self) -> u64 {
        self.peer_id
    }

    /// The authenticated peer role.
    #[inline]
    pub fn peer_role(&self) -> Role {
        self.peer_role
    }

    /// The negotiated cipher suite.
    #[inline]
    pub fn suite(&self) -> CipherSuite {
        self.suite
    }

    /// Seals `plaintext` (+ `aad`) into a fresh envelope.
    #[inline]
    pub fn seal(&self, plaintext: &[u8], aad: &[u8]) -> Result<Vec<u8>, SecError> {
        self.crypto.seal_to_vec(plaintext, aad)
    }

    /// Opens a sealed envelope into `out`, returning the plaintext length.
    #[inline]
    pub fn open(&self, sealed: &[u8], aad: &[u8], out: &mut [u8]) -> Result<usize, SecError> {
        self.crypto.open_into(sealed, aad, out)
    }

    /// Decrypts in place (zero-copy) — see [`CryptoBox::open_in_place`].
    #[inline]
    pub fn open_in_place(&self, buf: &mut [u8], aad: &[u8]) -> Result<usize, SecError> {
        self.crypto.open_in_place(buf, aad)
    }

    /// Shared crypto box (for ring-slot decryption helpers).
    #[inline]
    pub fn crypto(&self) -> &CryptoBox {
        &self.crypto
    }
}

/// HKDF info binding derived keys to this protocol version.
const HKDF_INFO: &[u8] = b"tpt-teleop-sec-v1";

fn generate_eph() -> Result<(EphemeralPrivateKey, [u8; 32]), SecError> {
    let rng = SystemRandom::new();
    let eph = EphemeralPrivateKey::generate(&X25519, &rng).map_err(|_| SecError::KeyGen)?;
    let pub_key = eph.compute_public_key().map_err(|_| SecError::KeyGen)?;
    let mut pub_bytes = [0u8; 32];
    pub_bytes.copy_from_slice(pub_key.as_ref());
    Ok((eph, pub_bytes))
}

fn derive_shared(our_eph: EphemeralPrivateKey, peer_pub: &[u8; 32]) -> Result<[u8; 32], SecError> {
    let peer = UnparsedPublicKey::new(&X25519, &peer_pub[..]);
    agreement::agree_ephemeral(our_eph, &peer, |shared| {
        let mut s = [0u8; 32];
        s.copy_from_slice(shared);
        s
    })
    .map_err(|_| SecError::Crypto)
}

fn derive_key(shared: &[u8; 32], suite: CipherSuite) -> CryptoBox {
    let salt = hkdf::Salt::new(hkdf::HKDF_SHA256, &[]);
    let prk = salt.extract(shared);
    let okm = prk
        .expand(&[HKDF_INFO], hkdf::HKDF_SHA256)
        .expect("32-byte HKDF expansion is always valid");
    let mut key = [0u8; 32];
    okm.fill(&mut key)
        .expect("fill into 32 bytes always succeeds");
    CryptoBox::from_kdf(suite, &key)
}

/// Begins a handshake: returns the initiator's first message and the pending
/// state (its ephemeral key) needed to finish.
pub fn begin_handshake(
    identity: &DeviceIdentity,
    suites: &[CipherSuite],
) -> Result<(HandshakeInit, PendingHandshake), SecError> {
    let (eph, eph_pub) = generate_eph()?;
    let attestation = Attestation::sign(identity, &eph_pub)?;
    Ok((
        HandshakeInit {
            attestation,
            suites: suites.to_vec(),
        },
        PendingHandshake { eph },
    ))
}

/// Responder side: verifies the initiator, derives the shared key, and returns
/// the reply message plus the established session.
pub fn respond_handshake(
    identity: &DeviceIdentity,
    init: &HandshakeInit,
    our_suites: &[CipherSuite],
) -> Result<(HandshakeResp, SecureSession), SecError> {
    if !init.attestation.verify() {
        return Err(SecError::AttestationFailed);
    }
    let (eph, eph_pub) = generate_eph()?;
    let shared = derive_shared(eph, &init.attestation.eph_pub)?;
    let suite = CipherSuite::negotiate(our_suites, &init.suites)
        .ok_or(SecError::Handshake("no common cipher suite"))?;
    let crypto = derive_key(&shared, suite);
    let attestation = Attestation::sign(identity, &eph_pub)?;
    let session = SecureSession {
        peer_id: init.attestation.unit_id,
        peer_role: init.attestation.role,
        crypto,
        suite,
    };
    Ok((HandshakeResp { attestation, suite }, session))
}

/// Initiator side: verifies the responder and finalizes the session.
pub fn finish_handshake(
    _identity: &DeviceIdentity,
    pending: PendingHandshake,
    resp: &HandshakeResp,
) -> Result<SecureSession, SecError> {
    if !resp.attestation.verify() {
        return Err(SecError::AttestationFailed);
    }
    let shared = derive_shared(pending.eph, &resp.attestation.eph_pub)?;
    let crypto = derive_key(&shared, resp.suite);
    Ok(SecureSession {
        peer_id: resp.attestation.unit_id,
        peer_role: resp.attestation.role,
        crypto,
        suite: resp.suite,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cipher::CipherSuite;

    fn suites() -> Vec<CipherSuite> {
        CipherSuite::all().to_vec()
    }

    #[test]
    fn full_handshake_yields_matching_keys() {
        let a = DeviceIdentity::generate(1, Role::Operator).unwrap();
        let b = DeviceIdentity::generate(2, Role::Admin).unwrap();

        let (init, pending) = begin_handshake(&a, &suites()).unwrap();
        let (resp, sess_b) = respond_handshake(&b, &init, &suites()).unwrap();
        let sess_a = finish_handshake(&a, pending, &resp).unwrap();

        // Peer identities are correctly learned.
        assert_eq!(sess_a.peer_id(), 2);
        assert_eq!(sess_a.peer_role(), Role::Admin);
        assert_eq!(sess_b.peer_id(), 1);
        assert_eq!(sess_b.peer_role(), Role::Operator);

        // The two boxes are key-synchronized: what A seals, B opens.
        assert_eq!(sess_a.suite(), sess_b.suite());
        let pt = b"take manual control";
        let sealed = sess_a.seal(pt, b"aad").unwrap();
        let mut out = [0u8; 64];
        let n = sess_b.open(&sealed, b"aad", &mut out).unwrap();
        assert_eq!(&out[..n], pt);

        // Cross-direction also works.
        let sealed2 = sess_b.seal(b"engage autonomy", b"aad2").unwrap();
        let mut out2 = [0u8; 64];
        let n2 = sess_a.open(&sealed2, b"aad2", &mut out2).unwrap();
        assert_eq!(&out2[..n2], b"engage autonomy");
    }

    #[test]
    fn rejected_attestation_fails_handshake() {
        let a = DeviceIdentity::generate(1, Role::Operator).unwrap();
        let b = DeviceIdentity::generate(2, Role::Admin).unwrap();
        let (init, _pending) = begin_handshake(&a, &suites()).unwrap();
        // Tamper with the attestation so verification fails.
        let mut bad = init.clone();
        bad.attestation.eph_pub[0] ^= 0xFF;
        assert!(respond_handshake(&b, &bad, &suites()).is_err());
    }

    #[test]
    fn no_common_suite_is_rejected() {
        let a = DeviceIdentity::generate(1, Role::Operator).unwrap();
        let b = DeviceIdentity::generate(2, Role::Admin).unwrap();
        let (init, _pending) = begin_handshake(&a, &[CipherSuite::Aes256Gcm]).unwrap();
        assert!(respond_handshake(&b, &init, &[CipherSuite::ChaCha20Poly1305]).is_err());
    }
}
