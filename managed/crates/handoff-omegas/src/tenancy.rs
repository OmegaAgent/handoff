//! `TenantResolver` — the tenant is the organization, and nothing else.
//!
//! The open core carries an opaque tenant reference and never learns what one *is*. Here it is an
//! Ωmegas `org_id`, and this adapter's whole job is to refuse anything that is not one.
//!
//! **A Space is not a tenant.** It was tempting to make `TenantRef` a `(org_id, space_id)` pair,
//! and it is wrong for a reason that is currently invisible: `space_grant()` returns `None`
//! unconditionally in the control plane today, so every org admin is an admin on every Space and
//! every member is an editor on every Space. Space-scoped isolation is therefore **not deliverable**
//! — "Finance's handoffs are invisible to Marketing" is a thing we cannot do — and encoding a Space
//! in the isolation key would produce a boundary that looks enforced and is not. The org is the
//! isolation boundary because the org is the boundary the control plane actually enforces. Space
//! becomes a filter on a read model when `space_grant()` starts returning grants, which is an
//! additive change to this file and to nothing else.

use handoff_core::auth::Principal;
use handoff_core::seam::TenantResolver;
use handoff_protocol::error::{ErrorCode, ProtocolError, Result};

/// The tenant is the org named by the credential.
#[derive(Debug, Clone, Copy, Default)]
pub struct OrgTenant;

impl TenantResolver for OrgTenant {
    fn tenant_of(&self, principal: &Principal) -> Result<String> {
        if principal.tenant_ref.is_empty() {
            return Err(ProtocolError::new(
                ErrorCode::InvalidApiKey,
                "the credential carries no organization, and this service will not infer one",
            ));
        }
        // The same check the receipt writer will make later. Failing here means a caller learns at
        // authentication time rather than when the first person answers.
        handoff_core::plan::tenant_as_org(&principal.tenant_ref)?;
        Ok(principal.tenant_ref.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use handoff_core::auth::PrincipalKind;
    use handoff_protocol::requires::{AuthStrength, Role};

    fn principal(tenant: &str) -> Principal {
        Principal {
            credential_ref: format!("{tenant}::test-credential"),
            id: None,
            kind: PrincipalKind::Machine,
            tenant_ref: tenant.to_string(),
            role: Role::Viewer,
            auth_strength: AuthStrength::Session,
            display: None,
            scopes: vec![],
        }
    }

    #[test]
    fn an_org_id_resolves_to_itself() {
        assert_eq!(
            OrgTenant
                .tenant_of(&principal("org_01K3M7QW8ZC4YRXB2N6VD9FTHE"))
                .unwrap(),
            "org_01K3M7QW8ZC4YRXB2N6VD9FTHE"
        );
    }

    #[test]
    fn a_space_id_is_not_a_tenant() {
        // Encoding a Space here would produce an isolation boundary the control plane does not
        // enforce, which is worse than not having one.
        assert!(OrgTenant
            .tenant_of(&principal("spc_01K3M7QW8ZC4YRXB2N6VD9FTHE"))
            .is_err());
    }

    #[test]
    fn an_empty_or_free_text_tenant_is_refused_rather_than_defaulted() {
        assert!(OrgTenant.tenant_of(&principal("")).is_err());
        assert!(OrgTenant.tenant_of(&principal("acme")).is_err());
        assert!(OrgTenant.tenant_of(&principal("default")).is_err());
    }
}
