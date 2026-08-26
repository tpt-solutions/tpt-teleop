//! Role-based access control (spec §5.5 / §5.6).
//!
//! Principals authenticate (via an [`crate::identity::Attestation`] verified by
//! the trust store) and are then authorized per-action against a static
//! role→permission policy. The same policy backs fleet-dispatch tools in
//! `tpt-t-cloud` (§5.6) and per-unit control decisions in `tpt-t-link`.

use std::collections::{HashMap, HashSet};

/// Operator / agent roles in the zero-trust model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    /// Unauthenticated or minimally authenticated caller.
    Guest,
    /// Read-only fleet/telemetry viewer.
    Observer,
    /// Human or AI operator able to issue commands.
    Operator,
    /// Fleet administrator (full control + management).
    Admin,
    /// Automated AI agent operating a unit (subject to shared-control policy).
    AiAgent,
}

impl Role {
    /// All roles, used to build default policies.
    pub fn all() -> &'static [Role] {
        &[
            Role::Guest,
            Role::Observer,
            Role::Operator,
            Role::Admin,
            Role::AiAgent,
        ]
    }
}

/// Discrete capabilities checked before any privileged action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Permission {
    /// Read telemetry / status.
    ViewTelemetry,
    /// Issue a control command on a unit.
    SendControl,
    /// Engage autonomous (Assist/Auto) mode.
    EngageAutonomy,
    /// Take manual (Full-Teleop) control.
    TakeManualControl,
    /// Manage fleet membership, units, assignments.
    ManageFleet,
    /// Read audit / security logs.
    AuditLog,
}

/// Static role→permission mapping. Cloning is cheap (small maps).
#[derive(Debug, Clone)]
pub struct Policy {
    grants: HashMap<Role, HashSet<Permission>>,
}

impl Default for Policy {
    /// Sensible zero-trust defaults: nobody gets more than explicitly listed.
    fn default() -> Self {
        let mut grants: HashMap<Role, HashSet<Permission>> = HashMap::new();
        grants.insert(Role::Guest, HashSet::new());
        grants.insert(
            Role::Observer,
            HashSet::from([Permission::ViewTelemetry, Permission::AuditLog]),
        );
        grants.insert(
            Role::Operator,
            HashSet::from([
                Permission::ViewTelemetry,
                Permission::SendControl,
                Permission::EngageAutonomy,
                Permission::TakeManualControl,
            ]),
        );
        grants.insert(
            Role::AiAgent,
            HashSet::from([
                Permission::ViewTelemetry,
                Permission::SendControl,
                Permission::EngageAutonomy,
            ]),
        );
        grants.insert(
            Role::Admin,
            HashSet::from([
                Permission::ViewTelemetry,
                Permission::SendControl,
                Permission::EngageAutonomy,
                Permission::TakeManualControl,
                Permission::ManageFleet,
                Permission::AuditLog,
            ]),
        );
        Self { grants }
    }
}

impl Policy {
    /// A default policy.
    pub fn new() -> Self {
        Self::default()
    }

    /// Replaces the permission set granted to a role.
    pub fn grant(&mut self, role: Role, perms: &[Permission]) {
        self.grants
            .entry(role)
            .or_default()
            .extend(perms.iter().copied());
    }

    /// Revokes every permission currently granted to a role.
    pub fn revoke_all(&mut self, role: Role) {
        if let Some(set) = self.grants.get_mut(&role) {
            set.clear();
        }
    }

    /// True iff `role` holds `perm`.
    pub fn role_has(&self, role: Role, perm: Permission) -> bool {
        self.grants.get(&role).is_some_and(|s| s.contains(&perm))
    }

    /// Authorizes a principal: authenticated, and at least one of its roles
    /// holds `perm`. Unauthenticated principals are always denied.
    pub fn authorize(&self, p: &Principal, perm: Permission) -> bool {
        if !p.authenticated {
            return false;
        }
        p.roles.iter().any(|r| self.role_has(*r, perm))
    }
}

/// An authenticated subject (human, agent, or device) presented for
/// authorization. Built from a verified attestation + role claim.
#[derive(Debug, Clone)]
pub struct Principal {
    /// Stable subject identifier (e.g. callsign or agent id).
    pub id: String,
    /// Roles claimed and accepted during authentication.
    pub roles: HashSet<Role>,
    /// Originating unit/device id (zero-trust: every action is attributable).
    pub device_id: u64,
    /// Whether the attestation chain was verified successfully.
    pub authenticated: bool,
}

impl Principal {
    /// An unauthenticated, role-less principal (denied everywhere).
    pub fn guest(id: &str) -> Self {
        Self {
            id: id.to_string(),
            roles: HashSet::new(),
            device_id: 0,
            authenticated: false,
        }
    }

    /// True iff the principal holds `role` among its accepted roles.
    pub fn has_role(&self, role: Role) -> bool {
        self.roles.contains(&role)
    }
}

/// Maps fleet-dispatch tool names (spec §5.6 MCP surface) to the permission
/// required to invoke them. Unknown tools map to `None` (always denied).
pub fn dispatch_permission(tool: &str) -> Option<Permission> {
    match tool {
        "list_units" => Some(Permission::ViewTelemetry),
        "get_telemetry" => Some(Permission::ViewTelemetry),
        "assign_unit" => Some(Permission::ManageFleet),
        "engage_autonomy" => Some(Permission::EngageAutonomy),
        "take_manual_control" => Some(Permission::TakeManualControl),
        "send_control" => Some(Permission::SendControl),
        "manage_fleet" => Some(Permission::ManageFleet),
        "read_audit" => Some(Permission::AuditLog),
        _ => None,
    }
}

/// Convenience: authorize a principal against a named dispatch tool.
pub fn authorize_dispatch(policy: &Policy, p: &Principal, tool: &str) -> bool {
    match dispatch_permission(tool) {
        Some(perm) => policy.authorize(p, perm),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn operator() -> Principal {
        let mut roles = HashSet::new();
        roles.insert(Role::Operator);
        Principal {
            id: "op1".into(),
            roles,
            device_id: 1,
            authenticated: true,
        }
    }

    #[test]
    fn operator_can_control_but_not_manage() {
        let policy = Policy::new();
        let op = operator();
        assert!(policy.authorize(&op, Permission::SendControl));
        assert!(policy.authorize(&op, Permission::TakeManualControl));
        assert!(!policy.authorize(&op, Permission::ManageFleet));
    }

    #[test]
    fn unauthenticated_denied_everywhere() {
        let policy = Policy::new();
        let mut op = operator();
        op.authenticated = false;
        assert!(!policy.authorize(&op, Permission::SendControl));
    }

    #[test]
    fn guest_has_no_privileges() {
        let policy = Policy::new();
        let g = Principal::guest("anon");
        for perm in [
            Permission::ViewTelemetry,
            Permission::SendControl,
            Permission::ManageFleet,
        ] {
            assert!(!policy.authorize(&g, perm));
        }
    }

    #[test]
    fn dispatch_tool_mapping() {
        let policy = Policy::new();
        let op = operator();
        assert!(authorize_dispatch(&policy, &op, "send_control"));
        assert!(authorize_dispatch(&policy, &op, "take_manual_control"));
        assert!(!authorize_dispatch(&policy, &op, "manage_fleet"));
        assert!(!authorize_dispatch(&policy, &op, "unknown_tool"));
    }

    #[test]
    fn admin_full_access() {
        let policy = Policy::new();
        let mut roles = HashSet::new();
        roles.insert(Role::Admin);
        let admin = Principal {
            id: "root".into(),
            roles,
            device_id: 0,
            authenticated: true,
        };
        for perm in [
            Permission::ViewTelemetry,
            Permission::SendControl,
            Permission::EngageAutonomy,
            Permission::TakeManualControl,
            Permission::ManageFleet,
            Permission::AuditLog,
        ] {
            assert!(policy.authorize(&admin, perm), "admin lacks {perm:?}");
        }
    }
}
