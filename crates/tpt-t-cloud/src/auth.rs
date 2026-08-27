//! Zero-trust admission for the fleet HTTP API and MCP dispatch (Phase 15).
//!
//! The cloud server is unauthenticated by default (legacy / LAN mode). When a
//! [`FleetAuthz`] gate is installed on the [`FleetServer`](crate::server::FleetServer)
//! every privileged route requires a verifiable [`Attestation`] presented in
//! the `Authorization` header; missing or insufficient attestation yields
//! `401` / `403` before any fleet mutation runs.
//!
//! Wire format: `Authorization: TPT-Attestation <HEX(Attestation::to_bytes())>`.

use crate::http::Request;
use crate::json::Json;
use tpt_t_sec::cipher::CipherSuite;
use tpt_t_sec::identity::Attestation;
use tpt_t_sec::rbac::Principal;
use tpt_t_sec::session::{HandshakeInit, HandshakeResp};
use tpt_t_sec::FleetAuthz;

/// Header scheme prefix for the attestation bearer.
const SCHEME: &str = "TPT-Attestation ";

/// Extracts an [`Attestation`] from a request's `Authorization` (or
/// `X-Tpt-Attestation`) header. Returns `None` when the header is absent or
/// malformed.
pub fn extract_attestation(req: &Request) -> Option<Attestation> {
    let value = req
        .headers
        .iter()
        .find(|(k, _)| k == "authorization" || k == "x-tpt-attestation")
        .map(|(_, v)| v.as_str())?;
    let hex = value.trim().strip_prefix(SCHEME).unwrap_or(value.trim());
    let wire = hex_decode(hex)?;
    Attestation::from_bytes(&wire).ok()
}

/// Authenticates a request into a [`Principal`].
///
/// * `Err(401)` — no attestation, or the attestation failed verification.
/// * `Err(403)` — authenticated but the gate denies (should not normally
///   arise at this stage; returned defensively).
/// * `Ok(principal)` — verified operator/agent.
pub fn authenticate_request(authz: &FleetAuthz, req: &Request) -> Result<Principal, u16> {
    match extract_attestation(req) {
        Some(att) => authz.authenticate(&att).map_err(|_| 401u16),
        None => Err(401),
    }
}

/// True iff `principal` is authorized for the named fleet-dispatch `tool`
/// under `authz`.
pub fn authorize_tool(authz: &FleetAuthz, principal: &Principal, tool: &str) -> bool {
    authz.authorize_dispatch(principal, tool)
}

/// Maps a [`CipherSuite`] to its compact wire number.
pub fn suite_to_u8(s: CipherSuite) -> u8 {
    match s {
        CipherSuite::Aes256Gcm => 1,
        CipherSuite::ChaCha20Poly1305 => 2,
    }
}

/// Inverse of [`suite_to_u8`].
pub fn suite_from_u8(n: u8) -> Option<CipherSuite> {
    match n {
        1 => Some(CipherSuite::Aes256Gcm),
        2 => Some(CipherSuite::ChaCha20Poly1305),
        _ => None,
    }
}

/// Decodes a JSON object into a [`HandshakeInit`] for the secure bootstrap
/// endpoint. Expected shape:
/// `{ "attestation": "<hex>", "suites": [1, 2] }`.
pub fn handshake_init_from_json(v: &Json) -> Option<HandshakeInit> {
    let att = v
        .get("attestation")
        .and_then(|a| a.as_str())
        .and_then(hex_decode)
        .and_then(|w| Attestation::from_bytes(&w).ok())?;
    let suites = v
        .get("suites")
        .and_then(|s| s.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|n| n.as_u64().and_then(|u| suite_from_u8(u as u8)))
                .collect::<Vec<_>>()
        })
        .unwrap_or_else(|| CipherSuite::all().to_vec());
    if suites.is_empty() {
        return None;
    }
    Some(HandshakeInit {
        attestation: att,
        suites,
    })
}

/// Encodes a [`HandshakeResp`] into JSON for the secure bootstrap endpoint.
pub fn handshake_resp_to_json(resp: &HandshakeResp) -> Json {
    Json::obj(&[
        (
            "attestation",
            Json::str(&hex_encode(&resp.attestation.to_bytes())),
        ),
        ("suite", Json::uint(suite_to_u8(resp.suite) as u64)),
    ])
}

/// Builds the `Authorization: TPT-Attestation <HEX(Attestation::to_bytes())>`
/// header value for a verified [`Attestation`] (used by clients and tests).
pub fn attestation_header(att: &Attestation) -> String {
    format!("Authorization: TPT-Attestation {}", hex_encode(&att.to_bytes()))
}

/// Minimal constant-time-ish hex decoder (uppercase/lowercase, even length).
fn hex_decode(s: &str) -> Option<Vec<u8>> {
    let bytes = s.as_bytes();
    if bytes.len() % 2 != 0 {
        return None;
    }
    let mut out = Vec::with_capacity(bytes.len() / 2);
    let mut i = 0;
    while i < bytes.len() {
        let hi = hex_val(bytes[i])?;
        let lo = hex_val(bytes[i + 1])?;
        out.push((hi << 4) | lo);
        i += 2;
    }
    Some(out)
}

/// Minimal hex encoder.
fn hex_encode(data: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(data.len() * 2);
    for &b in data {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0x0f) as usize] as char);
    }
    s
}

fn hex_val(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}
