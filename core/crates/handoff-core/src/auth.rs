//! Principals, and the two authority rules that are enforced here and nowhere else.
//!
//! The load-bearing one is §4.2: **a requester principal may never answer.** It is enforced by
//! principal *type*, which is why [`Principal::kind`] is an enum rather than a role string or a
//! scope. There is deliberately no configuration, no role, and no deployment mode that can turn a
//! [`PrincipalKind::Machine`] into an answerer — [`Principal::may_answer`] is total and has no
//! escape hatch.

use handoff_protocol::error::{ErrorCode, ProtocolError, Result};
use handoff_protocol::id::PrincipalId;
use handoff_protocol::requires::{AuthStrength, PresentedAuthority, Role};
use serde::{Deserialize, Serialize};

/// What kind of subject authenticated.
///
/// §4.1 names three: a machine holding an API key, a person, and a tenant administrator. The third
/// is a person with an administrative role rather than a separate kind, so this enum has the two
/// kinds that differ in *what they may do*, plus the anonymous grade §4.4 defines.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrincipalKind {
    /// An API key bound to a `service_account` subject. Raises, reads, routes, redeems — never
    /// answers (§4.2, I15).
    Machine,
    /// A person, authenticated as themselves.
    Human,
    /// Possession of a single-use delivery token and nothing else. **No person is identified**
    /// (§4.4). It may answer where the deployment permits the grade, and the receipt then records
    /// `actor.type = "anonymous_link"` and names nobody.
    AnonymousLink,
}

/// An authenticated caller.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Principal {
    /// The principal identity, absent for [`PrincipalKind::AnonymousLink`] — a Server MUST NOT
    /// record an identity it does not have.
    pub id: Option<PrincipalId>,
    /// Which kind of subject this is.
    pub kind: PrincipalKind,
    /// The tenant, resolved from stored state bound to the credential and never from a request
    /// body (§4.1, I13).
    pub tenant_ref: String,
    /// The role held right now. Authority is evaluated against this at answer time, not at raise
    /// time (§4.3).
    pub role: Role,
    /// The grade this caller actually authenticated at.
    pub auth_strength: AuthStrength,
    /// Display name, frozen onto a receipt at decision time.
    pub display: Option<String>,
    /// Scopes the credential carries. `*` means every scope.
    pub scopes: Vec<String>,
}

impl Principal {
    /// Whether this principal may ever answer a request.
    ///
    /// This is the whole of §4.2. It reads no role, no scope, and no configuration, because §4.2
    /// requires that no role, scope, setting, or deployment mode can grant a machine the power to
    /// answer.
    pub const fn may_answer(&self) -> bool {
        !matches!(self.kind, PrincipalKind::Machine)
    }

    /// Whether the credential carries a scope.
    pub fn has_scope(&self, scope: &str) -> bool {
        self.scopes.iter().any(|s| s == "*" || s == scope)
    }

    /// Refuse a caller that lacks a scope.
    pub fn require_scope(&self, scope: &str) -> Result<()> {
        if self.has_scope(scope) {
            Ok(())
        } else {
            Err(ProtocolError::new(
                ErrorCode::InsufficientScope,
                format!("this credential lacks `{scope}`"),
            ))
        }
    }

    /// What this caller presents to an authority evaluation (§4.3).
    ///
    /// `None` for an anonymous link, which has no principal to evaluate a role against; §4.4
    /// handles that grade separately so that nothing downstream can mistake it for a person.
    pub fn presented(&self) -> Option<PresentedAuthority> {
        self.id.map(|principal| PresentedAuthority {
            principal,
            role: self.role,
            auth_strength: self.auth_strength,
        })
    }
}

/// The deployment's answer to the question §4.4 leaves open.
///
/// This is separate from [`handoff_protocol::requires::DeploymentProfile`] on purpose, and C-6b is
/// why: a deployment that forbids `link_only` still **accepts a raise that declares it** and
/// refuses the *answer* at that grade. Folding the two together would reject the raise, and the
/// person who was going to be asked would never be asked at all.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AuthPolicy {
    /// Whether this deployment accepts `auth_strength: link_only` at all (§4.4).
    ///
    /// A deployment profile MAY forbid it entirely, in which case an answer at that grade is
    /// `403 auth_strength_not_permitted`. `link_only` exists in the model so that implementations
    /// which need it are honest about it, not so that it becomes a convenient default.
    pub link_only_permitted: bool,
}

impl AuthPolicy {
    /// Refuse a grade the deployment does not accept (§4.4, C-6b).
    ///
    /// Checked after the role, and with its own code: `insufficient_authority` would imply a role
    /// the caller could be granted, and this is not that.
    pub fn check_grade(&self, achieved: AuthStrength) -> Result<()> {
        if achieved == AuthStrength::LinkOnly && !self.link_only_permitted {
            return Err(ProtocolError::new(
                ErrorCode::AuthStrengthNotPermitted,
                "this deployment does not accept auth_strength=link_only",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn principal(kind: PrincipalKind) -> Principal {
        Principal {
            id: None,
            kind,
            tenant_ref: "org_00000000000000000000000000".into(),
            role: Role::Admin,
            auth_strength: AuthStrength::Mfa,
            display: None,
            scopes: vec!["*".into()],
        }
    }

    #[test]
    fn no_role_or_scope_lets_a_machine_answer() {
        let machine = principal(PrincipalKind::Machine);
        assert!(machine.has_scope("anything"));
        assert_eq!(machine.role, Role::Admin);
        assert!(!machine.may_answer());
    }

    #[test]
    fn people_and_anonymous_links_may_answer_subject_to_authority() {
        assert!(principal(PrincipalKind::Human).may_answer());
        assert!(principal(PrincipalKind::AnonymousLink).may_answer());
    }

    #[test]
    fn a_forbidden_grade_has_its_own_code() {
        let policy = AuthPolicy {
            link_only_permitted: false,
        };
        let err = policy.check_grade(AuthStrength::LinkOnly).unwrap_err();
        assert_eq!(err.code, ErrorCode::AuthStrengthNotPermitted);
        assert!(policy.check_grade(AuthStrength::Session).is_ok());
    }
}
