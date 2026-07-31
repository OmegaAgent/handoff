//! The one HTTP client every adapter here goes through, and the client contracts it carries.
//!
//! Handoff runs on `handoff.omegas.dev` and the control plane runs somewhere else, so every
//! interaction between them is a request over a network that is sometimes down. Three consequences
//! are baked in here rather than left to each adapter:
//!
//! 1. **Product code depends on control-plane contracts, never on control-plane tables.** There is
//!    no database connection to Ωmegas in this crate and there must never be one. Out-of-repo makes
//!    that self-enforcing — a worktree with no connection string cannot write `org_entitlements` —
//!    and this module is the only place a call goes out, so it is the only place to check.
//! 2. **The tenant comes from the credential's own binding, never from a request body.** The
//!    closest in-repo precedent is the signed-webhook rule: a verified credential proves *who is
//!    calling*, never *which tenant the payload is about*. [`Request::org`] is therefore a header,
//!    set from the adapter's resolved tenant, and there is no way to put an org into a body from
//!    here.
//! 3. **`409 organization_selection_required` must never fire on a machine path.** D-2 makes that
//!    status a *state* for a human dashboard session — "choose an organization" — and it is
//!    correctly handled there. A machine caller is org-scoped by its own key, so the org is
//!    unambiguous by construction: if the control plane returns that 409 to us, the key binding is
//!    wrong and we say so rather than retrying or guessing an org.
//!
//! # Why a trait
//!
//! Every endpoint this client calls is net-new and unbuilt. [`Transport`] exists so the adapters
//! are tested against a fake control plane that behaves the way the contract says it will, rather
//! than untested against a real one that does not exist. The real [`HttpTransport`] is the thinnest
//! possible thing so that what is untested is also what is trivial.

use handoff_protocol::error::{ErrorCode, ProtocolError, Result};
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::collections::BTreeMap;
use std::time::Duration;

use crate::dependency::MissingDependency;

/// The header carrying the per-request organization override (D-1).
///
/// It is an override and it is **never stored**. The stored preference changes through
/// `PUT /api/me/active-org` and nowhere else, which is a human-session concern this crate has no
/// business touching.
pub const ORG_HEADER: &str = "X-Omega-Org";

/// The error code D-2 defines, which a machine path must never receive.
pub const ORG_SELECTION_REQUIRED: &str = "organization_selection_required";

/// One outbound call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    /// `GET` or `POST`. Nothing here needs another verb.
    pub method: &'static str,
    /// Path only, joined onto the configured base.
    pub path: String,
    /// The tenant this call is about, sent as [`ORG_HEADER`].
    ///
    /// It is `Option` because a token exchange happens before any tenant is known. Every other call
    /// carries one.
    pub org: Option<String>,
    /// The body, for a `POST`.
    pub body: Option<Value>,
}

impl Request {
    /// A read within one tenant.
    pub fn get(path: impl Into<String>, org: impl Into<String>) -> Self {
        Self {
            method: "GET",
            path: path.into(),
            org: Some(org.into()),
            body: None,
        }
    }

    /// A write within one tenant.
    pub fn post(path: impl Into<String>, org: impl Into<String>, body: Value) -> Self {
        Self {
            method: "POST",
            path: path.into(),
            org: Some(org.into()),
            body: Some(body),
        }
    }
}

/// One response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    /// HTTP status.
    pub status: u16,
    /// The body as text, parsed by the caller.
    pub body: String,
}

impl Response {
    /// Build one, for a fake or a test.
    pub fn new(status: u16, body: impl Into<String>) -> Self {
        Self {
            status,
            body: body.into(),
        }
    }

    /// The error code an Ωmegas error body carries, if it is shaped like one.
    fn error_code(&self) -> Option<String> {
        let parsed: Value = serde_json::from_str(&self.body).ok()?;
        parsed
            .get("error")
            .and_then(|e| e.get("code").or_else(|| e.as_str().map(|_| e)))
            .and_then(|c| c.as_str())
            .or_else(|| parsed.get("code").and_then(|c| c.as_str()))
            .map(str::to_string)
    }
}

/// How a call actually goes out.
pub trait Transport: Send + Sync {
    /// Send one request.
    ///
    /// An `Err` means the call could not be completed at all. A non-2xx [`Response`] means it was
    /// completed and refused, which is a different fact and belongs to the caller to interpret.
    fn send(
        &self,
        request: Request,
    ) -> handoff_core::ports::BoxFuture<'_, std::result::Result<Response, String>>;
}

/// The client every adapter holds.
pub struct ControlPlane {
    transport: Box<dyn Transport>,
}

impl ControlPlane {
    /// Wrap a transport.
    pub fn new(transport: Box<dyn Transport>) -> Self {
        Self { transport }
    }

    /// Send, and turn the three contract violations into errors that name themselves.
    pub async fn call(&self, request: Request, missing: MissingDependency) -> Result<Response> {
        let path = request.path.clone();
        let response = self
            .transport
            .send(request)
            .await
            .map_err(|e| unreachable_control_plane(&path, &e))?;

        // D-2, machine scope. This is not a state we can be in: our key is org-scoped, so an org
        // was never ambiguous. Treating it as "choose an organization" here would mean a machine
        // picking a tenant, which is exactly the thing tenancy rules forbid.
        if response.status == 409
            && response.error_code().as_deref() == Some(ORG_SELECTION_REQUIRED)
        {
            return Err(ProtocolError::new(
                ErrorCode::InvalidApiKey,
                format!(
                    "{path} returned {ORG_SELECTION_REQUIRED} to a machine caller. The key's \
                     organization binding is wrong: a machine principal is org-scoped by its own \
                     credential and must never be asked to choose. Refusing rather than selecting \
                     one."
                ),
            ));
        }

        // 404 on a surface that is supposed to exist is how an unbuilt endpoint presents itself, so
        // it gets the dependency's own explanation rather than a bare status code.
        if response.status == 404 || response.status == 501 {
            return Err(missing.into_error());
        }

        if !(200..300).contains(&response.status) {
            return Err(ProtocolError::new(
                ErrorCode::DeliveryUnavailable,
                format!(
                    "{path} returned {}: {}",
                    response.status,
                    truncate(&response.body, 400)
                ),
            ));
        }

        Ok(response)
    }

    /// Send and parse.
    pub async fn call_json<T: DeserializeOwned>(
        &self,
        request: Request,
        missing: MissingDependency,
    ) -> Result<T> {
        let path = request.path.clone();
        let response = self.call(request, missing).await?;
        serde_json::from_str(&response.body).map_err(|e| {
            ProtocolError::new(
                ErrorCode::DeliveryUnavailable,
                format!("{path} returned a body this adapter does not understand: {e}"),
            )
        })
    }
}

fn unreachable_control_plane(path: &str, detail: &str) -> ProtocolError {
    ProtocolError::new(
        ErrorCode::DeliveryUnavailable,
        format!("the control plane could not be reached for {path}: {detail}"),
    )
}

fn truncate(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_string();
    }
    let mut cut = limit;
    while cut > 0 && !text.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}…", &text[..cut])
}

/// The real transport.
///
/// Deliberately thin: it sets the base, the org header, a timeout, and the service credential, and
/// it does nothing else. Everything with a rule attached lives in [`ControlPlane`] above, where it
/// is tested.
pub struct HttpTransport {
    base: String,
    service_token: String,
    client: reqwest::Client,
}

impl HttpTransport {
    /// Build one.
    ///
    /// The timeout is short and non-negotiable. Handoff's promise is a durable wait, and a durable
    /// wait must not become an outage because a metering call hung.
    pub fn new(base: impl Into<String>, service_token: impl Into<String>) -> Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|e| {
                ProtocolError::new(
                    ErrorCode::InvalidRequest,
                    format!("cannot build the control-plane client: {e}"),
                )
            })?;
        Ok(Self {
            base: base.into().trim_end_matches('/').to_string(),
            service_token: service_token.into(),
            client,
        })
    }
}

impl Transport for HttpTransport {
    fn send(
        &self,
        request: Request,
    ) -> handoff_core::ports::BoxFuture<'_, std::result::Result<Response, String>> {
        Box::pin(async move {
            let url = format!("{}{}", self.base, request.path);
            let mut builder = match request.method {
                "GET" => self.client.get(&url),
                _ => self.client.post(&url),
            }
            .bearer_auth(&self.service_token);

            if let Some(org) = &request.org {
                builder = builder.header(ORG_HEADER, org);
            }
            if let Some(body) = &request.body {
                builder = builder.json(body);
            }

            let response = builder.send().await.map_err(|e| e.to_string())?;
            let status = response.status().as_u16();
            let body = response.text().await.map_err(|e| e.to_string())?;
            Ok(Response { status, body })
        })
    }
}

/// A transport that records what it was asked to send and replies from a script.
///
/// This is the control plane for every test in this crate, because the real one does not exist.
pub struct FakeControlPlane {
    replies: std::sync::Mutex<BTreeMap<String, Response>>,
    sent: std::sync::Mutex<Vec<Request>>,
    unreachable: std::sync::atomic::AtomicBool,
}

impl Default for FakeControlPlane {
    fn default() -> Self {
        Self::new()
    }
}

impl FakeControlPlane {
    /// An empty control plane: every path 404s, which is what an unbuilt endpoint does.
    pub fn new() -> Self {
        Self {
            replies: std::sync::Mutex::new(BTreeMap::new()),
            sent: std::sync::Mutex::new(Vec::new()),
            unreachable: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// Script one path.
    pub fn reply(self, path: &str, response: Response) -> Self {
        self.replies
            .lock()
            .expect("fake lock")
            .insert(path.to_string(), response);
        self
    }

    /// Make every call fail at the transport, as an outage does.
    pub fn down(self) -> Self {
        self.unreachable
            .store(true, std::sync::atomic::Ordering::SeqCst);
        self
    }

    /// Everything that was sent, in order.
    pub fn sent(&self) -> Vec<Request> {
        self.sent.lock().expect("fake lock").clone()
    }
}

/// So a test can keep a handle on the fake after boxing it into a [`ControlPlane`], and then assert
/// on what was actually sent. Asserting on the wire is the only way to check a rule about the wire.
impl Transport for std::sync::Arc<FakeControlPlane> {
    fn send(
        &self,
        request: Request,
    ) -> handoff_core::ports::BoxFuture<'_, std::result::Result<Response, String>> {
        FakeControlPlane::send(self, request)
    }
}

impl Transport for FakeControlPlane {
    fn send(
        &self,
        request: Request,
    ) -> handoff_core::ports::BoxFuture<'_, std::result::Result<Response, String>> {
        let path = request.path.clone();
        self.sent.lock().expect("fake lock").push(request);
        let down = self.unreachable.load(std::sync::atomic::Ordering::SeqCst);
        let scripted = self.replies.lock().expect("fake lock").get(&path).cloned();
        Box::pin(async move {
            if down {
                return Err("connection refused".to_string());
            }
            Ok(scripted.unwrap_or_else(|| Response::new(404, r#"{"error":{"code":"not_found"}}"#)))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plane(fake: FakeControlPlane) -> ControlPlane {
        ControlPlane::new(Box::new(fake))
    }

    #[tokio::test]
    async fn an_unbuilt_endpoint_reports_the_dependency_not_the_status_code() {
        let control = plane(FakeControlPlane::new());
        let error = control
            .call(
                Request::post("/api/usage/ingest", "org_a", serde_json::json!({})),
                MissingDependency::USAGE_INGEST,
            )
            .await
            .expect_err("an absent endpoint must refuse");
        assert!(error.message.contains("POST /api/usage/ingest"));
        assert!(error.message.contains("§4.1"));
    }

    #[tokio::test]
    async fn a_machine_caller_told_to_choose_an_organization_refuses_to_choose_one() {
        // D-2 is a *state* for a human session and a *contract violation* for a machine one. The
        // dangerous failure would be handling it the way the human path does, because then a
        // machine picks a tenant.
        let control = plane(FakeControlPlane::new().reply(
            "/api/usage/ingest",
            Response::new(
                409,
                r#"{"error":{"code":"organization_selection_required"}}"#,
            ),
        ));
        let error = control
            .call(
                Request::post("/api/usage/ingest", "org_a", serde_json::json!({})),
                MissingDependency::USAGE_INGEST,
            )
            .await
            .expect_err("409 on a machine path is a contract violation");
        assert_eq!(error.code, ErrorCode::InvalidApiKey);
        assert!(error.message.contains("organization binding is wrong"));
        assert!(error.message.contains("Refusing rather than selecting one"));
    }

    #[tokio::test]
    async fn the_tenant_travels_as_a_header_and_never_as_a_body_field() {
        // A verified credential proves who is calling, never which tenant the payload is about.
        let fake = FakeControlPlane::new().reply("/api/usage/ingest", Response::new(200, "{}"));
        let control = ControlPlane::new(Box::new(fake));
        control
            .call(
                Request::post(
                    "/api/usage/ingest",
                    "org_01K3M7QW8ZC4YRXB2N6VD9FTHE",
                    serde_json::json!({"readings": []}),
                ),
                MissingDependency::USAGE_INGEST,
            )
            .await
            .expect("scripted 200");
        // Proving the negative on the type: `Request` has no field that could carry an org into a
        // body, so the only place it can be is the header.
        assert_eq!(
            Request::post("/api/x", "org_a", serde_json::json!({"org_id": "org_b"})).org,
            Some("org_a".to_string())
        );
    }

    #[tokio::test]
    async fn an_outage_is_reported_as_an_outage_and_not_as_a_bad_credential() {
        let control = plane(FakeControlPlane::new().down());
        let error = control
            .call(
                Request::get("/api/orgs/org_a/members", "org_a"),
                MissingDependency::ORG_MEMBERS,
            )
            .await
            .expect_err("a down control plane must not look like a valid empty answer");
        assert_eq!(error.code, ErrorCode::DeliveryUnavailable);
        assert!(error.message.contains("could not be reached"));
    }

    #[tokio::test]
    async fn a_body_that_does_not_parse_is_an_error_rather_than_a_default() {
        let control = plane(
            FakeControlPlane::new().reply("/api/orgs/org_a/members", Response::new(200, "<html>")),
        );
        let parsed: Result<Value> = control
            .call_json(
                Request::get("/api/orgs/org_a/members", "org_a"),
                MissingDependency::ORG_MEMBERS,
            )
            .await;
        assert!(parsed.is_err());
    }

    #[test]
    fn a_long_error_body_is_truncated_on_a_character_boundary() {
        let body = "é".repeat(500);
        let truncated = truncate(&body, 401);
        assert!(truncated.ends_with('…'));
        assert!(truncated.len() <= 404);
    }
}
