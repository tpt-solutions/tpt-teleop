//! Zero-trust broker: the "always verify" half of the model (spec §5.5).
//!
//! A unit is trusted only insofar as its Ed25519 signing key is enrolled in a
//! [`TrustStore`]. When a peer presents an [`Attestation`], the broker checks
//! (1) the signing key is enrolled for the claimed `unit_id`, and (2) the
//! signature is valid. A passing check yields an authenticated
//! [`Principal`] carrying the enrolled role — which the RBAC [`Policy`] then
//! authorizes per action.

use std::collections::HashMap;

use crate::error::SecError;
use crate::identity::Attestation;
use crate::rbac::{Policy, Principal, Role};

/// Enrolled signing keys, keyed by unit id. In a production fleet this is fed
/// by a root-of-trust / device-provisioning service; here it is an in-memory
/// map.
#[derive(Debug, Clone, Default)]
pub struct TrustStore {
    keys: HashMap<u64, [u8; 32]>,
    roles: HashMap<u64, Role>,
}

impl TrustStore {
    /// An empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Enrolls a unit's signing public key together with the role it is
    /// authorized to claim.
    pub fn enroll(&mut self, unit_id: u64, signing_pub: &[u8; 32], role: Role) {
        self.keys.insert(unit_id, *signing_pub);
        self.roles.insert(unit_id, role);
    }

    /// Number of enrolled units.
    pub fn len(&self) -> usize {
        self.keys.len()
    }

    /// True when no units are enrolled.
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    /// Verifies an attestation: the signing key must be enrolled for the
    /// claimed unit and the signature must be valid.
    pub fn verify(&self, att: &Attestation) -> Result<Role, SecError> {
        let enrolled = self.keys.get(&att.unit_id).ok_or(SecError::UnknownUnit)?;
        if enrolled != &att.signing_pub {
            return Err(SecError::AttestationFailed);
        }
        if !att.verify() {
            return Err(SecError::AttestationFailed);
        }
        Ok(self.roles.get(&att.unit_id).copied().unwrap_or(Role::Guest))
    }
}

/// The zero-trust broker: trust store + RBAC policy. The single entry point
/// the rest of the stack uses to turn a raw attestation into an
/// authorization decision.
#[derive(Debug, Clone)]
pub struct ZeroTrustBroker {
    trust: TrustStore,
    policy: Policy,
}

impl ZeroTrustBroker {
    /// Builds a broker from an existing trust store and policy.
    pub fn new(trust: TrustStore, policy: Policy) -> Self {
        Self { trust, policy }
    }

    /// Convenience builder with default policy.
    pub fn with_trust(trust: TrustStore) -> Self {
        Self {
            trust,
            policy: Policy::new(),
        }
    }

    /// Authenticates an attestation into a [`Principal`].
    pub fn authenticate(&self, att: &Attestation) -> Result<Principal, SecError> {
        let role = self.trust.verify(att)?;
        let mut roles = std::collections::HashSet::new();
        roles.insert(role);
        Ok(Principal {
            id: format!("unit-{}", att.unit_id),
            roles,
            device_id: att.unit_id,
            authenticated: true,
        })
    }

    /// Authorizes an authenticated principal for a permission.
    pub fn authorize(&self, p: &Principal, perm: crate::rbac::Permission) -> bool {
        self.policy.authorize(p, perm)
    }

    /// Authorizes a named fleet-dispatch tool (spec §5.6).
    pub fn authorize_dispatch(&self, p: &Principal, tool: &str) -> bool {
        crate::rbac::authorize_dispatch(&self.policy, p, tool)
    }

    /// Reference to the underlying policy (for offline checks / tests).
    pub fn policy(&self) -> &Policy {
        &self.policy
    }

    /// Reference to the trust store.
    pub fn trust(&self) -> &TrustStore {
        &self.trust
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::DeviceIdentity;

    #[test]
    fn broker_authenticates_and_authorizes() {
        let id = DeviceIdentity::generate(99, Role::Operator).unwrap();
        let mut trust = TrustStore::new();
        trust.enroll(99, id.public_key(), Role::Operator);
        let broker = ZeroTrustBroker::with_trust(trust);

        // Build a valid attestation (ephemeral key + signature).
        let eph = [0x11u8; 32];
        let att = Attestation::sign(&id, &eph).unwrap();

        let principal = broker.authenticate(&att).unwrap();
        assert!(principal.authenticated);
        assert_eq!(principal.device_id, 99);
        // Operators may control but not manage.
        assert!(broker.authorize_dispatch(&principal, "send_control"));
        assert!(!broker.authorize_dispatch(&principal, "manage_fleet"));
    }

    #[test]
    fn unknown_unit_is_rejected() {
        let id = DeviceIdentity::generate(5, Role::Admin).unwrap();
        let eph = [0x22u8; 32];
        let att = Attestation::sign(&id, &eph).unwrap();
        let broker = ZeroTrustBroker::with_trust(TrustStore::new());
        assert!(broker.authenticate(&att).is_err());
    }

    #[test]
    fn wrong_enrolled_key_is_rejected() {
        let id = DeviceIdentity::generate(5, Role::Admin).unwrap();
        let other = DeviceIdentity::generate(6, Role::Admin).unwrap();
        let mut trust = TrustStore::new();
        // Enroll a *different* key for unit 5.
        trust.enroll(5, other.public_key(), Role::Admin);
        let broker = ZeroTrustBroker::with_trust(trust);
        let eph = [0x33u8; 32];
        let att = Attestation::sign(&id, &eph).unwrap();
        assert!(broker.authenticate(&att).is_err());
    }
}
