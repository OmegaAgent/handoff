//! The deployment profile: everything the suite cannot know about a deployment it has never seen.
//!
//! The runner speaks only HTTP. Three kinds of fact are therefore outside it and must be supplied
//! by whoever is claiming conformance:
//!
//! 1. **Credentials.** Which token authenticates as a machine principal in tenant A, which as a
//!    human editor, which as an administrator in tenant B. Cases name principals by alias; the
//!    profile maps aliases to credentials.
//! 2. **Deployment choices the specification leaves open.** Whether `link_only` is permitted
//!    (§4.4), which channel declares `can_authenticate_person: false` (§4.7).
//! 3. **Hooks below the API.** C-15 must assert immutability *from the storage layer*, because the
//!    application is inside the threat model — so the deployment supplies the command that attempts
//!    the mutation, and the suite asserts that it was refused. C-7 must grep logs. C-21 must inject
//!    an inbound channel message. None of those are HTTP, and none of them belong in this crate.
//!
//! A profile is optional. Without one the suite still runs every case and every case fails with a
//! stated reason, which is the correct outcome: an unconfigured deployment has not demonstrated
//! anything.

use serde::Deserialize;
use std::collections::BTreeMap;

/// How a principal alias authenticates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrincipalKind {
    /// An API key bound to a `service_account` subject. Raises, reads, redeems — never answers.
    Machine,
    /// A human principal presenting a signed session assertion as a bearer token.
    HumanBearer,
    /// A human principal presenting a session cookie.
    HumanCookie,
    /// No credential at all.
    None,
}

/// One principal alias.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Principal {
    /// How this principal authenticates.
    pub kind: PrincipalKind,
    /// The credential. Absent for [`PrincipalKind::None`].
    #[serde(default)]
    pub token: Option<String>,
    /// The tenant this principal belongs to, for the operator's own reference. The suite never
    /// sends it: tenancy is resolved by the Server from stored state, never from a request (I13).
    #[serde(default)]
    pub org: Option<String>,
}

/// Callback-receiver settings for C-18.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CallbackConfig {
    /// Address the local receiver binds to. Port `0` picks a free one.
    #[serde(default)]
    pub bind: Option<String>,
    /// Host and port the Server should call back on, when the receiver is not directly reachable
    /// at its bind address (a tunnel, a container network). Defaults to the bind address.
    #[serde(default)]
    pub advertise: Option<String>,
    /// The current signing secret for this endpoint.
    #[serde(default)]
    pub secret: Option<String>,
    /// The previous signing secret, still inside a rotation overlap.
    #[serde(default)]
    pub secret_previous: Option<String>,
}

/// Everything the suite needs about the deployment under test.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    /// Base URL, including the `/v1` prefix. `--base-url` overrides it.
    #[serde(default)]
    pub base_url: Option<String>,

    /// Principal aliases the cases refer to.
    #[serde(default)]
    pub principals: BTreeMap<String, Principal>,

    /// Deployment choices the specification leaves open, as boolean flags a case can guard on.
    #[serde(default)]
    pub deployment: BTreeMap<String, serde_yaml::Value>,

    /// Shell commands, keyed by hook name, run with `sh -c`.
    #[serde(default)]
    pub hooks: BTreeMap<String, String>,

    /// Callback receiver settings.
    #[serde(default)]
    pub callback: CallbackConfig,

    /// Longest real sleep the suite may take when no `advance_clock` hook exists. A deployment that
    /// wants the TTL cases to run quickly supplies the hook instead of raising this.
    #[serde(default = "default_max_real_sleep")]
    pub max_real_sleep: String,

    /// Directory holding the specification's fixtures, relative to the repository root.
    #[serde(default = "default_fixtures_root")]
    pub fixtures_root: String,
}

fn default_max_real_sleep() -> String {
    "PT30S".to_string()
}

fn default_fixtures_root() -> String {
    "spec/fixtures".to_string()
}

impl Profile {
    /// Load a profile from a YAML file.
    pub fn load(path: &std::path::Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read profile {}: {e}", path.display()))?;
        serde_yaml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// Resolve a principal alias, or explain what is missing.
    pub fn principal(&self, alias: &str) -> Result<Principal, String> {
        if alias == "anonymous" {
            return Ok(Principal {
                kind: PrincipalKind::None,
                token: None,
                org: None,
            });
        }
        self.principals.get(alias).cloned().ok_or_else(|| {
            format!(
                "no credential for principal `{alias}`. Add it under `principals:` in the \
                 deployment profile and pass --profile."
            )
        })
    }

    /// Resolve a hook command, or explain what is missing.
    pub fn hook(&self, name: &str) -> Result<String, String> {
        self.hooks.get(name).cloned().ok_or_else(|| {
            format!(
                "no `{name}` hook. This assertion is below the HTTP API by design, so the \
                 deployment must supply the command under `hooks:` in its profile."
            )
        })
    }

    /// Read a deployment flag as a boolean.
    pub fn flag(&self, name: &str) -> Result<bool, String> {
        match self.deployment.get(name) {
            Some(serde_yaml::Value::Bool(b)) => Ok(*b),
            Some(other) => Err(format!(
                "deployment flag `{name}` is not a boolean: {other:?}"
            )),
            None => Err(format!(
                "no deployment flag `{name}`. The specification leaves this choice to the \
                 deployment, so the profile must state it under `deployment:`."
            )),
        }
    }
}
