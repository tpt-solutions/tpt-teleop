# Plan: Replace `ring` (Apache-2.0 AND ISC) with RustCrypto (MIT-only chain)

## Context

`ring 0.17` is the only crypto primitive wired into `tpt-t-sec` (Phase 11 / spec §5.5),
providing AES-256-GCM + ChaCha20-Poly1305 AEAD, X25519 ECDH, HKDF-SHA256, Ed25519, and an
OS RNG. `ring` is licensed **`Apache-2.0 AND ISC`**, which breaks the project's strict
§2 MIT-chain policy (no Apache-2.0) and forced a `deny.toml` exception.

Decision (user-approved): **swap `ring` for RustCrypto + dalek crates**, which are
`MIT OR Apache-2.0` (resolves to MIT under the existing policy, identical to how `rkyv`/`libc`/
`syn` are already handled) — or MIT-only (`*-dalek`). This removes the cargo-deny exception,
keeps the MIT chain pure, and drops `ring`'s C/`cc` build step (pure-Rust → simpler
cross-compilation for the Linux/macOS/Windows matrix).

All `ring` usage is confined to `crates/tpt-t-sec` (verified by grep: `cipher.rs`,
`session.rs`, `identity.rs`, `error.rs`). No other crate touches `ring::` — `tpt-t-cloud` /
`tpt-t-link` only use the public `CryptoBox`/`SecureSession` API, so no downstream edits needed.

## Files to change

1. `crates/tpt-t-sec/Cargo.toml` — drop `ring = "0.17"`; add RustCrypto/dalek deps.
2. `crates/tpt-t-sec/src/cipher.rs` — AEAD + RNG (in-place seal/open preserved).
3. `crates/tpt-t-sec/src/session.rs` — X25519 ECDH + HKDF.
4. `crates/tpt-t-sec/src/identity.rs` — Ed25519 sign/verify + RNG.
5. `crates/tpt-t-sec/src/error.rs` — replace `From<ring::error::*>` with RustCrypto/dalek `From` impls.
6. `deny.toml` — remove the `[[licenses.exceptions]]` block for `ring` added during the audit.

## New dependencies (`crates/tpt-t-sec/Cargo.toml`)

```toml
# All dual MIT OR Apache-2.0 (→ MIT under policy) unless noted.
aes-gcm = "0.10"          # AES-256-GCM, impls aead::AeadInPlace
chacha20poly1305 = "0.10" # ChaCha20-Poly1305, impls aead::AeadInPlace
aead = "0.5"              # AeadInPlace, KeyInit, Nonce, Tag, Aad
hkdf = "0.12"             # HKDF-SHA256 (pulls hmac + sha2, both dual MIT/Apache)
getrandom = "0.2"         # OS RNG (fills [u8;N]); dual MIT/Apache
x25519-dalek = "2"        # X25519 ECDH; MIT
ed25519-dalek = "2"       # Ed25519; MIT
```
Remove the now-stale comment about `cc`/`ring` in `[dependencies]`.

> Implementer note: confirm exact dalek 2.x constructor names against the compiler; seed
> keys via `getrandom` into a `[u8;32]` and construct `StaticSecret::from(bytes)` /
> `SigningKey::from_bytes(&arr)` to avoid pulling `rand`/`OsRng`. If a needed dalek dep
> turns out Apache-only, `cargo deny check` will catch it — substitute the RustCrypto
> equivalent or a MIT-only crate before proceeding.

## API mapping (preserve the on-wire envelope `nonce 12B ‖ ct ‖ tag 16B`)

### `cipher.rs`
- Constants: `NONCE_LEN = 12`, `TAG_LEN = 16` (both suites are 96-bit NONCE / 128-bit TAG).
- `CryptoBox` inner key: replace the single `ring::aead::LessSafeKey` with an enum so the
  suite is chosen at runtime without heap alloc on the hot path:
  ```rust
  enum Inner { Aes(aes_gcm::Aes256Gcm), Cha(chacha20poly1305::ChaCha20Poly1305) }
  ```
  `CipherSuite::algorithm()` → build the matching `Inner` (both via `new_from_slice(key)`,
  mapping `aead::Error` → `SecError::InvalidKeyLength`).
- RNG salt: replace `SystemRandom::new()` + `rng.fill(&mut salt)` with
  `getrandom::getrandom(&mut salt).map_err(|_| SecError::KeyGen)?`.
- Seal (keep `seal_in_place_separate_tag` wire format, allocation-free):
  ```rust
  let nonce = aead::Nonce::from_slice(&nonce_bytes);
  let tag = inner.encrypt_in_place_detached(nonce, aad, &mut out[NONCE..NONCE+ptlen])?;
  out[NONCE+ptlen..need].copy_from_slice(&tag);
  ```
  (RustCrypto `AeadInPlace::encrypt_in_place_detached` encrypts `buffer` in place and returns
  the detached `Tag` — exact analogue of `seal_in_place_separate_tag`.)
- Open: `inner.decrypt_in_place_detached(nonce, aad, &mut buf[NONCE..NONCE+ptlen], tag)` →
  then `buf.copy_within(NONCE..NONCE+ptlen, 0)` to slide plaintext to front (unchanged).
- `seal_to_vec` / `open_into` / `decrypt_into_ring` / `open_in_place`: unchanged signatures.

### `session.rs`
- `use ring::agreement / hkdf / rand::SystemRandom` →
  `use x25519_dalek::{StaticSecret, PublicKey};` `use hkdf::Hkdf;` `use sha2::Sha256;`
  `use getrandom::getrandom;`.
- `generate_eph`: `let mut seed=[0u8;32]; getrandom(&mut seed)?; let sk=StaticSecret::from(seed);`
  `let pk = PublicKey::from(&sk);` return `(sk, pk.to_bytes())`.
  (`PendingHandshake` holds `StaticSecret` instead of `EphemeralPrivateKey`.)
- `derive_shared(our_sk, peer_pub)`: `our_sk.diffie_hellman(&PublicKey::from(peer_pub))`
  `.as_bytes().clone()` → `[u8;32]`.
- `derive_key`: `let h = Hkdf::<Sha256>::new(None, shared); h.expand(&[HKDF_INFO], &mut key)
  .map_err(|_| SecError::Crypto)?;` (replace `hkdf::Salt/extract/expand/fill`).
- `begin_handshake` / `respond_handshake` / `finish_handshake`: unchanged except the type
  behind `PendingHandshake`.

### `identity.rs`
- `use ring::rand::SystemRandom; use ring::signature::{...}` →
  `use ed25519_dalek::{SigningKey, VerifyingKey, Signature}; use getrandom::getrandom;`.
- `DeviceIdentity::generate`: `let mut seed=[0u8;32]; getrandom(&mut seed)?;`
  `let signing = SigningKey::from_bytes(&seed);` `signing_pub = signing.verifying_key().to_bytes();`
- `Attestation::sign`: `let sig: [u8;64] = identity.signing.sign(&msg).to_bytes();`
- `Attestation::verify` / `verify_message`:
  `VerifyingKey::from_bytes(&pub)?.verify(msg, &Signature::from_bytes(&sig)?).is_ok()`
  (handle the `from_bytes` `Result`).
- `sign_message`: `self.signing.sign(msg).to_bytes()`.

### `error.rs`
- Remove `impl From<ring::error::Unspecified>` and `From<ring::error::KeyRejected>`.
- Add: `impl From<aead::Error> for SecError { fn from(_) -> Self { SecError::Crypto } }`
  (covers seal/open key/tag failures) and `impl From<hkdf::InvalidLength> for SecError`
  → `SecError::Crypto`. Map dalek construction/verify errors to `SecError::Crypto` /
  `SecError::InvalidKeyLength` at call sites.

### `deny.toml`
- Delete the `[[licenses.exceptions]]` block granting `ring` `Apache-2.0` (added during the
  audit). No other changes needed — dual MIT/Apache crates resolve to MIT automatically.

## Validation (must all be green before tagging v1.0.0)

1. `cargo build -p tpt-t-sec` — compiles with new deps.
2. `cargo test -p tpt-t-sec` — all existing AEAD / handshake / identity tests still pass
   (they pin the wire format and forward-secret handshake; no behavior change expected).
3. `cargo test --workspace` — integration `e2e_pipeline` + `zero_alloc` (which pulls
   `tpt-t-sec` via `tpt-t-cloud`/`tpt-t-link`? verify no regression) still green.
4. `cargo clippy --workspace --all-targets -- -D warnings` — no new warnings.
5. `cargo fmt --all -- --check` — formatted.
6. `cargo deny check` — **must report `licenses ok` with NO exception and NO Apache-2.0**
   (this is the whole point of the swap).

## Risks / open questions
- dalek 2.x exact API names (`StaticSecret::from`, `PublicKey::from`, `SigningKey::from_bytes`,
  `VerifyingKey::from_bytes` returning `Result`) — follow compiler + existing tests.
- If any transitive dep of the dalek crates is Apache-only, `cargo deny check` fails; resolve
  by pinning a MIT-compatible version or swapping the crate (RustCrypto `p256`/`ed25519` trait
  + an alternate impl exist as fallbacks).
- `getrandom` in `no_std`/WASM targets may need features; this workspace targets
  Linux/macOS/Windows natives, so default features are fine.
