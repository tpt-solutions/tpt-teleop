//! Integration with `tpt-t-cloud` (spec §5.6): fleet dispatch authorization.
//!
//! The cloud server admits no unit or operator without a verified
//! [`Attestation`] and a matching RBAC grant. [`FleetAuthz`] is the gate the
//! fleet-management / MCP dispatch surface calls before any tool
//! (`list_units`, `engage_autonomy`, `take_manual_control`, …) runs.
//!
//! It is a thin, dependency-free composition of the zero-trust
//! [`ZeroTrustBroker`](crate::zerotrust::ZeroTrustBroker) (attestation →
//! authenticated [`Principal`]) and the [`Policy`] (principal → permission).

use crate::identity::Attestation;
use crate::rbac::{Permission, Policy, Principal};
use crate::zerotrust::{TrustStore, ZeroTrustBroker};

/// Fleet-wide authorization gate (spec §5.6).
#[derive(Debug, Clone)]
pub struct FleetAuthz {
    broker: ZeroTrustBroker,
}

impl FleetAuthz {
    /// Builds a gate from an enrolled [`TrustStore`] and an RBAC [`Policy`].
    pub fn new(trust: TrustStore, policy: Policy) -> Self {
        Self {
            broker: ZeroTrustBroker::new(trust, policy),
        }
    }

    /// Convenience builder with the default policy.
    pub fn with_trust(trust: TrustStore) -> Self {
        Self {
            broker: ZeroTrustBroker::with_trust(trust),
        }
    }

    /// Authenticates a unit/operator attestation into a [`Principal`].
    pub fn authenticate(&self, att: &Attestation) -> Result<Principal, crate::error::SecError> {
        self.broker.authenticate(att)
    }

    /// Authorizes a generic permission for an authenticated principal.
    pub fn authorize(&self, p: &Principal, perm: Permission) -> bool {
        self.broker.authorize(p, perm)
    }

    /// Authorizes a named fleet-dispatch tool (e.g. `engage_autonomy`).
    pub fn authorize_dispatch(&self, p: &Principal, tool: &str) -> bool {
        self.broker.authorize_dispatch(p, tool)
    }

    /// One-shot: authenticate an attestation and authorize a tool in a single
    /// call. Returns `false` on either failure (zero-trust: deny by default).
    pub fn admit(&self, att: &Attestation, tool: &str) -> bool {
        match self.authenticate(att) {
            Ok(principal) => self.authorize_dispatch(&principal, tool),
            Err(_) => false,
        }
    }

    /// Underlying broker (for composing with session establishment).
    pub fn broker(&self) -> &ZeroTrustBroker {
        &self.broker
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::DeviceIdentity;
    use crate::rbac::Role;
    use crate::session::begin_handshake;

    #[test]
    fn cloud_gate_admits_operator_control_rejects_admin_tools() {
        let op = DeviceIdentity::generate(11, Role::Operator).unwrap();
        let admin = DeviceIdentity::generate(12, Role::Admin).unwrap();

        let mut trust = TrustStore::new();
        trust.enroll(11, op.public_key(), Role::Operator);
        trust.enroll(12, admin.public_key(), Role::Admin);
        let gate = FleetAuthz::with_trust(trust);

        // Operator can engage autonomy but not manage fleet.
        let eph = [0xABu8; 32];
        let op_att = Attestation::sign(&op, &eph).unwrap();
        assert!(gate.admit(&op_att, "engage_autonomy"));
        assert!(gate.admit(&op_att, "take_manual_control"));
        assert!(!gate.admit(&op_att, "manage_fleet"));

        // Admin can do everything.
        let eph2 = [0xCDu8; 32];
        let admin_att = Attestation::sign(&admin, &eph2).unwrap();
        assert!(gate.admit(&admin_att, "manage_fleet"));
        assert!(gate.admit(&admin_att, "engage_autonomy"));
    }

    #[test]
    fn cloud_gate_rejects_unenrolled() {
        let op = DeviceIdentity::generate(21, Role::Operator).unwrap();
        let eph = [0x11u8; 32];
        let att = Attestation::sign(&op, &eph).unwrap();
        let gate = FleetAuthz::with_trust(TrustStore::new());
        assert!(!gate.admit(&att, "engage_autonomy"));
    }

    #[test]
    fn cloud_gate_ties_to_session_peer_role() {
        // A handshake proves identity; the cloud then checks the established
        // session's peer role against policy before dispatching.
        let op = DeviceIdentity::generate(31, Role::Operator).unwrap();
        let srv = DeviceIdentity::generate(32, Role::Admin).unwrap();
        let (init, pending) = begin_handshake(&op, crate::cipher::CipherSuite::all()).unwrap();
        let (resp, _srv_session) =
            crate::session::respond_handshake(&srv, &init, crate::cipher::CipherSuite::all())
                .unwrap();
        let _op_session = crate::session::finish_handshake(&op, pending, &resp).unwrap();

        // The server enrolls the operator's public key and gates dispatch on
        // the attestation the operator used to start the handshake.
        let mut trust = TrustStore::new();
        trust.enroll(31, op.public_key(), Role::Operator);
        let gate = FleetAuthz::with_trust(trust);
        assert!(gate.admit(&init.attestation, "send_control"));
    }
}
