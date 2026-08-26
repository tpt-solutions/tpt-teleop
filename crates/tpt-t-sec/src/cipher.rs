//! AEAD wrappers over `ring` (spec §5.5): AES-256-GCM and ChaCha20-Poly1305,
//! with zero-copy seal/open paths that decrypt directly into a
//! [`tpt_t_ring`] slot.
//!
//! The wire envelope produced by every seal is:
//!
//! ```text
//! ┌──────────┬──────────────────────────┬──────────┐
//! │ nonce 12B │ ciphertext (var)         │ tag 16B  │
//! └──────────┴──────────────────────────┴──────────┘
//! ```
//!
//! `open_in_place` decrypts the ciphertext+tag region in place and slides the
//! recovered plaintext to the front of the caller's buffer, so the result can
//! be pushed straight into an [`SpscRing`] slot with no heap allocation.

use core::sync::atomic::{AtomicU64, Ordering};

use ring::aead;
use ring::rand::{SecureRandom, SystemRandom};

use crate::error::SecError;

/// AEAD nonce length (both suites are 96-bit NONCE).
pub const NONCE_LEN: usize = aead::NONCE_LEN;
/// AEAD authentication tag length (both suites are 128-bit).
pub const TAG_LEN: usize = aead::MAX_TAG_LEN;

/// Negotiable AEAD cipher suite.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CipherSuite {
    /// AES-256 in Galois/Counter Mode.
    Aes256Gcm,
    /// ChaCha20 with Poly1305 MAC (preferred where AES-NI is unavailable).
    ChaCha20Poly1305,
}

impl CipherSuite {
    /// The `ring` algorithm backing this suite.
    #[inline]
    fn algorithm(&self) -> &'static aead::Algorithm {
        match self {
            CipherSuite::Aes256Gcm => &aead::AES_256_GCM,
            CipherSuite::ChaCha20Poly1305 => &aead::CHACHA20_POLY1305,
        }
    }

    /// Maximum per-message overhead (nonce + tag) in bytes.
    #[inline]
    pub fn overhead(&self) -> usize {
        NONCE_LEN + TAG_LEN
    }

    /// All suites the implementation can negotiate, most-preferred first.
    pub fn all() -> &'static [CipherSuite] {
        &[CipherSuite::Aes256Gcm, CipherSuite::ChaCha20Poly1305]
    }

    /// Picks the best suite both sides advertise (first common entry of
    /// `ours` intersected with `theirs`, preserving our preference order).
    pub fn negotiate(ours: &[CipherSuite], theirs: &[CipherSuite]) -> Option<CipherSuite> {
        ours.iter().copied().find(|s| theirs.contains(s))
    }
}

/// A symmetric session box: a single key, a chosen suite, and a monotonic
/// nonce counter (96-bit = 64-bit counter ‖ 32-bit random salt) so the same
/// key never encrypts two messages under the same nonce.
///
/// Intentionally `!Sync`-ish: it is owned by one thread (the network thread,
/// exactly like `NetService`), and the nonce counter is the only shared
/// mutable state — an atomic, lock-free.
pub struct CryptoBox {
    key: aead::LessSafeKey,
    suite: CipherSuite,
    ctr: AtomicU64,
    salt: [u8; 4],
}

impl CryptoBox {
    /// Builds a box from exactly the right number of raw key bytes
    /// (32 for either suite).
    pub fn new(suite: CipherSuite, key: &[u8]) -> Result<Self, SecError> {
        let unbound = aead::UnboundKey::new(suite.algorithm(), key)?;
        let rng = SystemRandom::new();
        let mut salt = [0u8; 4];
        rng.fill(&mut salt).map_err(|_| SecError::KeyGen)?;
        Ok(Self {
            key: aead::LessSafeKey::new(unbound),
            suite,
            ctr: AtomicU64::new(0),
            salt,
        })
    }

    /// Derives a box from any 32-byte slice already produced by a KDF.
    #[inline]
    pub fn from_kdf(suite: CipherSuite, kdf: &[u8; 32]) -> Self {
        // UnboundKey::new only fails on length, and 32 is always valid here.
        let unbound = aead::UnboundKey::new(suite.algorithm(), kdf)
            .expect("32-byte key is valid for either suite");
        let rng = SystemRandom::new();
        let mut salt = [0u8; 4];
        let _ = rng.fill(&mut salt);
        Self {
            key: aead::LessSafeKey::new(unbound),
            suite,
            ctr: AtomicU64::new(0),
            salt,
        }
    }

    /// The negotiated suite.
    #[inline]
    pub fn suite(&self) -> CipherSuite {
        self.suite
    }

    /// Overhead in bytes added by seal (nonce + tag).
    #[inline]
    pub fn overhead(&self) -> usize {
        self.suite.overhead()
    }

    fn next_nonce(&self, out: &mut [u8; NONCE_LEN]) {
        let v = self.ctr.fetch_add(1, Ordering::Relaxed);
        out[..8].copy_from_slice(&v.to_le_bytes());
        out[8..].copy_from_slice(&self.salt);
    }

    /// Seals `plaintext` (with `aad`) into a freshly allocated envelope.
    /// Convenience for tests and non-hot paths.
    pub fn seal_to_vec(&self, plaintext: &[u8], aad: &[u8]) -> Result<Vec<u8>, SecError> {
        let mut out = vec![0u8; NONCE_LEN + plaintext.len() + TAG_LEN];
        let n = self.seal_into(plaintext, aad, &mut out)?;
        out.truncate(n);
        Ok(out)
    }

    /// Seals `plaintext` into `out`, which must hold at least
    /// `plaintext.len() + overhead()` bytes. Returns the written length.
    ///
    /// No heap allocation occurs; the caller supplies the destination (often a
    /// pre-allocated datagram or ring scratch buffer).
    pub fn seal_into(
        &self,
        plaintext: &[u8],
        aad: &[u8],
        out: &mut [u8],
    ) -> Result<usize, SecError> {
        let need = NONCE_LEN + plaintext.len() + TAG_LEN;
        if out.len() < need {
            return Err(SecError::BufferTooSmall);
        }
        let mut nonce = [0u8; NONCE_LEN];
        self.next_nonce(&mut nonce);
        out[..NONCE_LEN].copy_from_slice(&nonce);
        out[NONCE_LEN..NONCE_LEN + plaintext.len()].copy_from_slice(plaintext);
        let n = aead::Nonce::try_assume_unique_for_key(&nonce)?;
        // Seal in place over the ciphertext‖tag region using the separate-tag
        // form so a caller-owned `out` (not a growable Vec) can be used —
        // this keeps the seal path allocation-free on the hot path.
        let tag = self.key.seal_in_place_separate_tag(
            n,
            aead::Aad::from(aad),
            &mut out[NONCE_LEN..NONCE_LEN + plaintext.len()],
        )?;
        out[NONCE_LEN + plaintext.len()..need].copy_from_slice(tag.as_ref());
        Ok(need)
    }

    /// Opens a sealed envelope from `sealed` into `out` (which must hold at
    /// least `sealed.len() - overhead()` bytes). Returns the plaintext length.
    ///
    /// Convenience wrapper: it copies the envelope into a scratch buffer,
    /// opens it in place, and slides the plaintext into `out`. The zero-copy
    /// hot path is [`open_in_place`](Self::open_in_place) /
    /// [`decrypt_into_ring`], which decrypt directly into a ring slot.
    pub fn open_into(&self, sealed: &[u8], aad: &[u8], out: &mut [u8]) -> Result<usize, SecError> {
        if sealed.len() < NONCE_LEN + TAG_LEN {
            return Err(SecError::Crypto);
        }
        let pt_len = sealed.len() - NONCE_LEN - TAG_LEN;
        if out.len() < pt_len {
            return Err(SecError::BufferTooSmall);
        }
        let mut work = sealed.to_vec();
        let n = self.open_in_place(&mut work, aad)?;
        out[..n].copy_from_slice(&work[..n]);
        Ok(n)
    }

    /// Opens a sealed envelope **in place**: `buf` must start with the 12-byte
    /// nonce followed by ciphertext‖tag. On success the recovered plaintext is
    /// slid to the front of `buf` and its length is returned. This is the
    /// zero-copy path used when decrypting straight into a ring slot.
    pub fn open_in_place(&self, buf: &mut [u8], aad: &[u8]) -> Result<usize, SecError> {
        if buf.len() < NONCE_LEN + TAG_LEN {
            return Err(SecError::Crypto);
        }
        let nonce = aead::Nonce::try_assume_unique_for_key(&buf[..NONCE_LEN])?;
        let pt = self
            .key
            .open_in_place(nonce, aead::Aad::from(aad), &mut buf[NONCE_LEN..])?;
        let pt_len = pt.len();
        // Slide plaintext to the front so the whole block is contiguous and
        // ready to be pushed into an SpscRing slot.
        buf.copy_within(NONCE_LEN..NONCE_LEN + pt_len, 0);
        Ok(pt_len)
    }
}

/// A fixed-capacity, inline plaintext block for zero-copy reception. A
/// decrypted payload lives entirely inside `buf` (no heap), so pushing the
/// block into an [`SpscRing`] moves it between threads with a single copy and
/// zero allocation.
#[repr(C, align(8))]
pub struct SecureBlock<const N: usize> {
    /// Recovered plaintext (first `len` bytes valid).
    pub buf: [u8; N],
    /// Valid byte count.
    pub len: u32,
}

impl<const N: usize> SecureBlock<N> {
    /// An empty block.
    pub fn new() -> Self {
        Self {
            buf: [0u8; N],
            len: 0,
        }
    }

    /// The recovered plaintext slice.
    #[inline]
    pub fn as_slice(&self) -> &[u8] {
        &self.buf[..self.len as usize]
    }
}

impl<const N: usize> Default for SecureBlock<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> Clone for SecureBlock<N> {
    fn clone(&self) -> Self {
        let mut b = Self::new();
        b.buf = self.buf;
        b.len = self.len;
        b
    }
}

/// Capacity of the largest inline secure block the stack ships by default
/// (covers a sealed `ControlCommand` plus margin).
pub const MAX_SECURE_BLOCK: usize = 256;

/// Decrypts `sealed` directly into a fresh [`SecureBlock`] and pushes it onto
/// `ring`. Returns the plaintext length, or [`SecError::RingFull`] if the ring
/// could not accept the block. This is the spec §5.5 "zero-copy decrypt
/// directly into `tpt-t-ring`" path.
pub fn decrypt_into_ring<const N: usize>(
    box_: &CryptoBox,
    sealed: &[u8],
    aad: &[u8],
    ring: &tpt_t_ring::SpscRing<SecureBlock<N>>,
) -> Result<usize, SecError> {
    if sealed.len() > N {
        return Err(SecError::BufferTooSmall);
    }
    let mut block = SecureBlock::<N>::new();
    block.buf[..sealed.len()].copy_from_slice(sealed);
    let pt = box_.open_in_place(&mut block.buf[..sealed.len()], aad)?;
    block.len = pt as u32;
    ring.push(block).map_err(|_| SecError::RingFull)?;
    Ok(pt)
}

#[cfg(test)]
mod tests {
    use super::*;

    const AAD: &[u8] = b"tpt-teleop";

    fn kdf(suite: CipherSuite) -> CryptoBox {
        let mut k = [0u8; 32];
        for (i, b) in k.iter_mut().enumerate() {
            *b = (i as u8).wrapping_mul(7).wrapping_add(3);
        }
        CryptoBox::from_kdf(suite, &k)
    }

    #[test]
    fn aes_gcm_roundtrip() {
        let box_ = kdf(CipherSuite::Aes256Gcm);
        let pt = b"engage autonomy now";
        let sealed = box_.seal_to_vec(pt, AAD).unwrap();
        assert_eq!(sealed.len(), pt.len() + box_.overhead());
        let mut out = vec![0u8; pt.len()];
        let n = box_.open_into(&sealed, AAD, &mut out).unwrap();
        assert_eq!(n, pt.len());
        assert_eq!(&out[..n], pt);
    }

    #[test]
    fn chacha_roundtrip() {
        let box_ = kdf(CipherSuite::ChaCha20Poly1305);
        let pt = b"telemetry burst 1234567890";
        let sealed = box_.seal_to_vec(pt, AAD).unwrap();
        let mut out = vec![0u8; pt.len()];
        let n = box_.open_into(&sealed, AAD, &mut out).unwrap();
        assert_eq!(&out[..n], pt);
    }

    #[test]
    fn wrong_aad_fails() {
        let box_ = kdf(CipherSuite::Aes256Gcm);
        let sealed = box_.seal_to_vec(b"hello", AAD).unwrap();
        let mut out = [0u8; 16];
        assert!(box_.open_into(&sealed, b"other", &mut out).is_err());
    }

    #[test]
    fn tampered_ciphertext_fails() {
        let box_ = kdf(CipherSuite::ChaCha20Poly1305);
        let mut sealed = box_.seal_to_vec(b"payload", AAD).unwrap();
        let last = sealed.len() - 1;
        sealed[last] ^= 0xFF;
        let mut out = [0u8; 16];
        assert!(box_.open_into(&sealed, AAD, &mut out).is_err());
    }

    #[test]
    fn zero_copy_into_ring() {
        let box_ = kdf(CipherSuite::Aes256Gcm);
        let ring: tpt_t_ring::SpscRing<SecureBlock<MAX_SECURE_BLOCK>> =
            tpt_t_ring::SpscRing::with_capacity(8);
        let sealed = box_.seal_to_vec(b"ring payload", AAD).unwrap();
        let n = decrypt_into_ring(&box_, &sealed, AAD, &ring).unwrap();
        assert_eq!(n, b"ring payload".len());
        let block = ring.pop().expect("block present");
        assert_eq!(block.as_slice(), b"ring payload");
    }

    #[test]
    fn in_place_open_slides_plaintext_to_front() {
        let box_ = kdf(CipherSuite::ChaCha20Poly1305);
        let pt = b"abcdefghij";
        let sealed = box_.seal_to_vec(pt, AAD).unwrap();
        let mut buf = [0u8; 64];
        buf[..sealed.len()].copy_from_slice(&sealed);
        let n = box_.open_in_place(&mut buf[..sealed.len()], AAD).unwrap();
        assert_eq!(&buf[..n], pt);
    }

    #[test]
    fn nonce_never_reused() {
        let box_ = kdf(CipherSuite::Aes256Gcm);
        let mut prev = [0u8; NONCE_LEN];
        for _ in 0..1024 {
            let sealed = box_.seal_to_vec(b"x", AAD).unwrap();
            assert_ne!(&sealed[..NONCE_LEN], &prev[..]);
            prev.copy_from_slice(&sealed[..NONCE_LEN]);
        }
    }

    #[test]
    fn suite_negotiation_prefers_local_order() {
        assert_eq!(
            CipherSuite::negotiate(
                &[CipherSuite::ChaCha20Poly1305, CipherSuite::Aes256Gcm],
                CipherSuite::all()
            ),
            Some(CipherSuite::ChaCha20Poly1305)
        );
        assert_eq!(
            CipherSuite::negotiate(&[CipherSuite::Aes256Gcm], &[CipherSuite::ChaCha20Poly1305]),
            None
        );
    }
}
