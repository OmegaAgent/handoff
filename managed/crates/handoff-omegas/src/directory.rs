//! `RecipientDirectory` — org members, over HTTP.
//!
//! `GET /api/orgs/{id}/members` is planned as an org surface, and this reads it. What it cannot read
//! is a **contact point**: no per-person phone, handle, or verified address exists in any of the
//! control plane's 74 migrations. That is not an oversight in this adapter; it is a gap in the thing
//! this adapter consumes.
//!
//! The consequence is stated rather than hidden. Without contact records the managed directory can
//! route to exactly one channel — the in-app surface, where a person authenticates as themselves —
//! and it says so instead of returning people with empty contact lists that read as "nobody has an
//! email address". A caller who asks this directory to page someone gets a refusal naming
//! [`MissingDependency::CONTACT_POINTS`], not a silent no-op.
//!
//! Two smaller rules, both from §7.5 and both easy to get wrong:
//!
//! - **A rotation resolves at rung-fire time**, not at raise time, which is why this is a call and
//!   not a snapshot. Whoever is on call at 3am is not whoever was on call when the agent asked.
//! - **An empty result is a legitimate answer, not an error.** A role with no members or a rotation
//!   with nobody on it is a real state; §7.3 requires the request to survive it, because failing the
//!   raise would put a directory outage inside the caller's agent.

use handoff_core::ports::BoxFuture;
use handoff_core::seam::{ContactPoint, Recipient, RecipientDirectory};
use handoff_protocol::error::{ErrorCode, ProtocolError, Result};
use handoff_protocol::id::PrincipalId;
use handoff_protocol::requires::{Target, TargetKind};
use serde::Deserialize;
use std::sync::Arc;

use crate::control_plane::{ControlPlane, Request};
use crate::dependency::MissingDependency;

/// The in-app surface, the one channel the managed tier can route to without a contact record.
///
/// A person opens the request surface and authenticates there, which is why it is also the only
/// starter channel that can carry an answer at all.
pub const IN_APP: &str = "inapp";

#[derive(Debug, Deserialize)]
struct MembersBody {
    #[serde(default)]
    members: Vec<Member>,
}

#[derive(Debug, Deserialize)]
struct Member {
    /// `usr_…`.
    id: String,
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    timezone: Option<String>,
    #[serde(default)]
    role: Option<String>,
    /// Per-person contact records, when the control plane has any.
    ///
    /// It has none today, in any of its 74 migrations. The field is read rather than assumed absent
    /// so that the day it starts arriving, this adapter uses it instead of quietly ignoring it.
    #[serde(default)]
    contacts: Vec<ContactPoint>,
}

/// The org-backed directory.
pub struct OmegasDirectory {
    control: Arc<ControlPlane>,
    /// Whether a per-person contact record exists to read.
    ///
    /// `false` today, and it must stay `false` until the control plane actually has one. Flipping
    /// this to `true` before the record exists would turn a loud refusal into a silent
    /// non-delivery, which is the failure this whole adapter is arranged to avoid.
    contact_points_available: bool,
}

impl OmegasDirectory {
    /// Build one.
    pub fn new(control: Arc<ControlPlane>, contact_points_available: bool) -> Self {
        Self {
            control,
            contact_points_available,
        }
    }

    async fn members(&self, tenant: &str) -> Result<Vec<Member>> {
        let body: MembersBody = self
            .control
            .call_json(
                Request::get(format!("/api/orgs/{tenant}/members"), tenant),
                MissingDependency::ORG_MEMBERS,
            )
            .await?;
        Ok(body.members)
    }
}

fn matches(target: &Target, member: &Member) -> bool {
    match target.kind {
        TargetKind::Principal => member.id == target.value,
        TargetKind::Role => member.role.as_deref() == Some(target.value.as_str()),
        // A group and a rotation are both control-plane concepts that do not exist yet. Matching
        // nobody is the correct answer for a set we cannot enumerate — inventing a fallback that
        // matched *everyone* would page an entire organization for a group that was never defined.
        TargetKind::Group | TargetKind::Rotation => false,
        TargetKind::Anyone => true,
    }
}

impl RecipientDirectory for OmegasDirectory {
    fn resolve(&self, tenant: String, target: Target) -> BoxFuture<'_, Result<Vec<Recipient>>> {
        Box::pin(async move {
            let members = self.members(&tenant).await?;
            let mut resolved = Vec::new();
            for member in members.iter().filter(|m| matches(&target, m)) {
                let Ok(id) = PrincipalId::parse(&member.id) else {
                    // A member the protocol cannot name is not a member we can put on a receipt.
                    // Skipping is right; guessing an identifier is not.
                    tracing::warn!(member = %member.id, "org member is not a principal identifier");
                    continue;
                };
                // The in-app surface authenticates the person itself, so its address is verified by
                // construction rather than by a confirmation flow. Every member has it, always.
                let mut contacts = vec![ContactPoint {
                    channel: IN_APP.into(),
                    address: member.id.clone(),
                    verified: true,
                }];
                if self.contact_points_available {
                    if member.contacts.is_empty() {
                        // A deployment that claims to have contact records and returns none is
                        // misconfigured, and the symptom of tolerating it is the exact failure this
                        // adapter exists to avoid: deliveries that go nowhere and report nothing.
                        return Err(ProtocolError::new(
                            ErrorCode::DeliveryUnavailable,
                            format!(
                                "this deployment is configured as having per-person contact \
                                 records, but the directory returned none for {}. Refusing rather \
                                 than routing to a person we cannot reach.",
                                member.id
                            ),
                        ));
                    }
                    contacts.extend(member.contacts.iter().cloned());
                }
                resolved.push(Recipient {
                    principal_id: Some(id),
                    display: member.display_name.clone(),
                    timezone: member.timezone.clone(),
                    contacts,
                    // Quiet hours need a per-person preference record, which is the same missing
                    // table as contact points. `None` means "we do not know", never "always awake".
                    quiet_hours: None,
                });
            }
            Ok(resolved)
        })
    }
}

impl OmegasDirectory {
    /// Whether this directory can reach a person on a channel other than the in-app surface.
    ///
    /// A caller asking to page someone should call this and get the refusal, rather than
    /// discovering the gap as an undelivered request an hour later.
    pub fn can_route_to(&self, channel: &str) -> Result<()> {
        if channel == IN_APP || self.contact_points_available {
            return Ok(());
        }
        Err(MissingDependency::CONTACT_POINTS.into_error())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control_plane::{FakeControlPlane, Response};

    fn directory(fake: FakeControlPlane) -> OmegasDirectory {
        OmegasDirectory::new(Arc::new(ControlPlane::new(Box::new(fake))), false)
    }

    const ORG: &str = "org_01K3M7QW8ZC4YRXB2N6VD9FTHE";

    fn members_body() -> Response {
        Response::new(
            200,
            serde_json::json!({
                "members": [
                    {"id": "usr_01K3M7QW8ZC4YRXB2N6VD9FTHE", "display_name": "Dana",
                     "timezone": "Europe/Berlin", "role": "admin"},
                    {"id": "usr_01K3M7QW8ZC4YRXB2N6VD9FTHF", "display_name": "Ola",
                     "role": "member"},
                    {"id": "not-a-principal", "role": "member"}
                ]
            })
            .to_string(),
        )
    }

    fn fake() -> FakeControlPlane {
        FakeControlPlane::new().reply(&format!("/api/orgs/{ORG}/members"), members_body())
    }

    fn target(kind: TargetKind, value: &str) -> Target {
        Target {
            kind,
            value: value.into(),
        }
    }

    #[tokio::test]
    async fn anyone_resolves_to_every_member_the_protocol_can_name() {
        let resolved = directory(fake())
            .resolve(ORG.into(), target(TargetKind::Anyone, "*"))
            .await
            .expect("resolve");
        // Length and identity, never `contains`: a directory that returns one extra person pages one
        // extra person.
        assert_eq!(resolved.len(), 2);
        assert_eq!(resolved[0].display.as_deref(), Some("Dana"));
        assert_eq!(resolved[1].display.as_deref(), Some("Ola"));
    }

    #[tokio::test]
    async fn a_role_target_resolves_to_that_role_alone() {
        let resolved = directory(fake())
            .resolve(ORG.into(), target(TargetKind::Role, "admin"))
            .await
            .expect("resolve");
        assert_eq!(resolved.len(), 1);
        assert_eq!(
            resolved[0].principal_id.map(|id| id.to_string()).as_deref(),
            Some("usr_01K3M7QW8ZC4YRXB2N6VD9FTHE")
        );
    }

    #[tokio::test]
    async fn a_group_or_rotation_resolves_to_nobody_rather_than_to_everybody() {
        // The dangerous fallback: treat an unknown set as "anyone". That pages an entire
        // organization for a group that was never defined.
        for kind in [TargetKind::Group, TargetKind::Rotation] {
            let resolved = directory(fake())
                .resolve(ORG.into(), target(kind, "on-call"))
                .await
                .expect("resolve");
            assert_eq!(resolved.len(), 0);
        }
    }

    #[tokio::test]
    async fn an_org_with_no_members_is_an_empty_answer_and_not_an_error() {
        let empty = FakeControlPlane::new().reply(
            &format!("/api/orgs/{ORG}/members"),
            Response::new(200, r#"{"members":[]}"#),
        );
        let resolved = directory(empty)
            .resolve(ORG.into(), target(TargetKind::Anyone, "*"))
            .await
            .expect("an empty roster is a real state");
        assert!(resolved.is_empty());
    }

    #[tokio::test]
    async fn every_resolved_person_can_be_reached_in_app_and_nowhere_else() {
        let resolved = directory(fake())
            .resolve(ORG.into(), target(TargetKind::Anyone, "*"))
            .await
            .expect("resolve");
        for person in &resolved {
            assert_eq!(person.contacts.len(), 1);
            assert_eq!(person.contacts[0].channel, IN_APP);
        }
    }

    #[tokio::test]
    async fn claiming_to_have_contact_records_and_returning_none_is_refused() {
        // Flipping the flag before the table exists must not degrade to silent non-delivery.
        let directory = OmegasDirectory::new(Arc::new(ControlPlane::new(Box::new(fake()))), true);
        let error = directory
            .resolve(ORG.into(), target(TargetKind::Anyone, "*"))
            .await
            .expect_err("a person we cannot reach is not a person we route to");
        assert!(error.message.contains("Refusing rather than routing"));
    }

    #[test]
    fn asking_to_page_someone_is_refused_with_the_missing_record_named() {
        let directory = directory(fake());
        assert!(directory.can_route_to(IN_APP).is_ok());
        let error = directory
            .can_route_to("voice")
            .expect_err("there is no phone number to call");
        assert!(error.message.contains("contact record"));
        assert!(error.message.contains("07:319"));
    }

    #[tokio::test]
    async fn an_absent_members_endpoint_names_the_dependency() {
        let error = directory(FakeControlPlane::new())
            .resolve(ORG.into(), target(TargetKind::Anyone, "*"))
            .await
            .expect_err("the endpoint does not exist yet");
        assert!(error.message.contains("/api/orgs/{id}/members"));
    }
}
