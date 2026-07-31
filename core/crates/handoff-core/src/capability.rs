//! Capabilities: blast radius, and the provider registry that declares it.
//!
//! §11.1 is the constraint the whole module is shaped by: **the protocol never carries a resolvable
//! address by value, anywhere.** A grant is a handle plus a description. The address exists only in
//! the response to an authenticated resolve, is minted per session, and is stored in no table here
//! — which is why [`ResolvedTransport`] has no persistence and no identifier of its own.
//!
//! Adding a capability kind — a remote desktop, a phone bridge, a document editor — registers a
//! provider. It adds no branch: nothing in this module matches on `provider` or `resource_ref`, and
//! §11.1 says explicitly that the core must not.

use handoff_protocol::error::{ErrorCode, ProtocolError, Result};
use handoff_protocol::receipt::{digest_of, Digest};
use handoff_protocol::requires::{CapabilityDeclaration, CapabilityScope};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The scope of consequence a person accepts when they take a capability (§11.5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlastRadius {
    /// One sentence a non-expert understands. MUST be rendered before the accept control.
    pub summary: String,
    /// The one field the core can **compare**, which is why it is a closed vocabulary while
    /// everything else here is opaque provider text.
    pub shared_with: SharedWith,
    /// How many people's access is implicated. A count, never a roster.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub principals: Option<u32>,
    /// What the surface is signed in as.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub identities: Vec<BlastRadiusIdentity>,
    /// Whether actions taken through this capability can be undone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reversible: Option<bool>,
    /// Any additional consequence the person should know.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// How widely the consequence is shared. Closed, because policy compares it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SharedWith {
    /// Nothing else is affected.
    Isolated,
    /// Only this request's own resources.
    Request,
    /// A shared workspace.
    Space,
    /// The whole tenant.
    Org,
}

/// One identity the capability is signed in as.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlastRadiusIdentity {
    /// Where the identity applies. Origin-level; never a full URL with parameters.
    pub origin: String,
    /// The account as a person would name it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

impl BlastRadius {
    /// The digest that binds a resolve to what the person was shown (§11.5 rule 2).
    pub fn digest(&self) -> Result<Digest> {
        digest_of(&serde_json::to_value(self).map_err(|e| {
            ProtocolError::new(
                ErrorCode::InvalidRequest,
                format!("blast radius is not serializable: {e}"),
            )
        })?)
    }
}

/// The address a resolved session connects to.
///
/// It exists in one response body and nowhere else: not in a table, not in an event, not in a log
/// line, not in a message. There is deliberately no `Serialize` round trip back into storage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedTransport {
    /// Transport family, so a client picks its connection method without parsing the URL.
    pub kind: TransportKind,
    /// Single-session and short-lived. Treat it as a secret in flight and discard it on release.
    pub url: String,
}

/// Transport family.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransportKind {
    /// A bidirectional socket.
    Websocket,
    /// Plain request/response.
    Https,
}

/// What a provider contributes to a capability the core carries but does not understand.
///
/// The core calls this; it never matches on the provider's name. That is the difference between a
/// registry and a switch statement, and §11.1 requires the former.
pub trait CapabilityProvider: Send + Sync {
    /// The scope of consequence a person accepts, for one declaration.
    fn blast_radius(&self, declaration: &CapabilityDeclaration) -> BlastRadius;

    /// Mint the one resolvable address in the system, bound to a single session.
    ///
    /// Called at resolve time only, and the result is never handed back to this trait to store.
    fn transport(
        &self,
        declaration: &ProviderResource,
        session_ref: &str,
        nonce: &str,
    ) -> ResolvedTransport;
}

/// What a provider is told about the resource it is being asked to open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderResource {
    /// The capability kind.
    pub capability_type: String,
    /// Opaque provider resource id, handed straight back to the provider.
    pub resource_ref: Option<String>,
    /// The scopes this session was granted.
    pub scopes: Vec<CapabilityScope>,
}

/// The provider every deployment has before it registers any of its own.
///
/// It declares a conservative blast radius derived from the declaration itself, and mints an
/// ephemeral per-session address under a configured base. A deployment that actually operates a
/// live surface replaces it by registering a provider under the same name.
#[derive(Debug, Clone)]
pub struct EphemeralProvider {
    /// Scheme and authority the minted address sits under, e.g. `wss://surfaces.example.com`.
    pub transport_base: String,
}

impl CapabilityProvider for EphemeralProvider {
    fn blast_radius(&self, declaration: &CapabilityDeclaration) -> BlastRadius {
        let subject = declaration.label.clone().unwrap_or_else(|| {
            format!("the {} this request declares", declaration.capability_type)
        });
        BlastRadius {
            summary: match declaration.scope {
                CapabilityScope::Drive => {
                    format!("Full control of {subject}, and everything it is signed into")
                }
                CapabilityScope::View => format!("A view of {subject}, with no input accepted"),
            },
            shared_with: SharedWith::Request,
            principals: Some(1),
            identities: Vec::new(),
            reversible: Some(declaration.scope == CapabilityScope::View),
            note: declaration.purpose.clone(),
        }
    }

    fn transport(
        &self,
        _resource: &ProviderResource,
        session_ref: &str,
        nonce: &str,
    ) -> ResolvedTransport {
        let base = self.transport_base.trim_end_matches('/');
        ResolvedTransport {
            kind: if base.starts_with("ws") {
                TransportKind::Websocket
            } else {
                TransportKind::Https
            },
            url: format!("{base}/surfaces/{session_ref}?t={nonce}"),
        }
    }
}

/// Providers by name, with a fallback for declarations that name none.
///
/// Lookup, never a match: a new provider is an entry here and zero changes anywhere else.
pub struct CapabilityRegistry {
    providers: BTreeMap<String, Box<dyn CapabilityProvider>>,
    fallback: Box<dyn CapabilityProvider>,
}

impl CapabilityRegistry {
    /// A registry with only the fallback provider.
    pub fn new(fallback: Box<dyn CapabilityProvider>) -> Self {
        Self {
            providers: BTreeMap::new(),
            fallback,
        }
    }

    /// Register a provider under the name declarations will call it by.
    pub fn register(&mut self, name: impl Into<String>, provider: Box<dyn CapabilityProvider>) {
        self.providers.insert(name.into(), provider);
    }

    /// Resolve a provider by name, falling back rather than failing: an unknown **provider** is a
    /// deployment that has not registered one yet, whereas an unknown **capability type** fails
    /// closed at parse time (§19, C-16).
    pub fn provider(&self, name: Option<&str>) -> &dyn CapabilityProvider {
        name.and_then(|n| self.providers.get(n))
            .map_or(&*self.fallback, |p| &**p)
    }
}

impl std::fmt::Debug for CapabilityRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CapabilityRegistry")
            .field("providers", &self.providers.keys().collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use handoff_protocol::id::GrantHandle;

    fn declaration(scope: CapabilityScope) -> CapabilityDeclaration {
        CapabilityDeclaration {
            handle: GrantHandle::from_random([7u8; 16]),
            capability_type: "interactive_surface".into(),
            scope,
            provider: None,
            resource_ref: Some("opaque:bs_4KpQ".into()),
            label: Some("the browser the agent is driving".into()),
            purpose: None,
            optional: false,
            ttl: None,
            blast_radius_digest: None,
        }
    }

    #[test]
    fn a_drive_grant_says_so_in_the_summary_a_person_reads() {
        let provider = EphemeralProvider {
            transport_base: "wss://surfaces.example".into(),
        };
        let radius = provider.blast_radius(&declaration(CapabilityScope::Drive));
        assert!(radius.summary.contains("Full control"));
        assert_eq!(radius.reversible, Some(false));
        assert_eq!(radius.shared_with, SharedWith::Request);
    }

    #[test]
    fn the_digest_changes_when_the_radius_does() {
        let provider = EphemeralProvider {
            transport_base: "wss://surfaces.example".into(),
        };
        let view = provider.blast_radius(&declaration(CapabilityScope::View));
        let drive = provider.blast_radius(&declaration(CapabilityScope::Drive));
        assert_ne!(view.digest().unwrap(), drive.digest().unwrap());
    }

    #[test]
    fn two_resolves_of_one_grant_mint_different_addresses() {
        let provider = EphemeralProvider {
            transport_base: "wss://surfaces.example".into(),
        };
        let resource = ProviderResource {
            capability_type: "interactive_surface".into(),
            resource_ref: None,
            scopes: vec![CapabilityScope::Drive],
        };
        let a = provider.transport(&resource, "hs_A", "nonce-a");
        let b = provider.transport(&resource, "hs_B", "nonce-b");
        assert_ne!(a.url, b.url);
        assert!(a.url.starts_with("wss://"));
    }
}
