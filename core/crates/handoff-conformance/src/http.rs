//! The HTTP client and the traffic recorder.
//!
//! Every byte the suite sends or receives is recorded, because two of the cases are **scans**
//! rather than unit tests: C-7 requires that a secret value appears in no artifact the scenario
//! produced, and C-8 requires the same of a resolvable grant URL. An assertion that only inspects
//! the response you happened to look at proves nothing about the response you did not.

use crate::profile::{Principal, PrincipalKind};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

/// One recorded exchange.
#[derive(Debug, Clone)]
pub struct Exchange {
    /// Method and full URL, query string included — the scan searches URLs separately, because
    /// §12.3 forbids a secret value in a query string specifically.
    pub url: String,
    /// HTTP method.
    pub method: String,
    /// Request headers as sent, including the credential.
    pub request_headers: BTreeMap<String, String>,
    /// Request body as sent.
    pub request_body: Option<String>,
    /// Response status.
    pub status: u16,
    /// Response headers.
    pub response_headers: BTreeMap<String, String>,
    /// Response body as received.
    pub response_body: String,
}

/// A response, whatever its status. A non-2xx is data here, not an error: most of this suite is
/// about asserting the *right* failure.
#[derive(Debug, Clone)]
pub struct Response {
    /// HTTP status.
    pub status: u16,
    /// Response headers, lowercased keys.
    pub headers: BTreeMap<String, String>,
    /// Raw body.
    pub body: String,
}

impl Response {
    /// Parse the body as JSON, or return a null document when it is empty or not JSON. A `204`
    /// legitimately has no body, and an implementation returning HTML on error should fail on the
    /// assertion that names the missing field rather than on a parse error that names nothing.
    pub fn json(&self) -> serde_json::Value {
        if self.body.trim().is_empty() {
            return serde_json::Value::Null;
        }
        serde_json::from_str(&self.body).unwrap_or(serde_json::Value::Null)
    }

    /// The body as a document to assert against, or the reason it is not one.
    ///
    /// [`Response::json`] answers `null` for a body that does not parse, which is right for a
    /// reader and wrong for a matcher: every path resolves to nothing against `null`, so every
    /// negative assertion — `not_contains_text`, `exists: false` — passes against a stack trace,
    /// an HTML error page, or a truncated response. A step that asserts on members therefore parses
    /// through here, where a body that is not JSON is the failure it obviously is.
    pub fn document(&self) -> Result<serde_json::Value, String> {
        let raw = self.body.trim();
        if raw.is_empty() {
            return Ok(serde_json::Value::Null);
        }
        serde_json::from_str(raw).map_err(|e| {
            let head: String = raw.chars().take(200).collect();
            format!(
                "the response body is not JSON ({e}), and this step asserts against its members. \
                 Against a body that did not parse every path resolves to nothing, which satisfies \
                 every assertion about something being absent.\n      body: {head}"
            )
        })
    }

    /// Headers as a JSON object, so the same matchers work on them.
    pub fn headers_json(&self) -> serde_json::Value {
        serde_json::Value::Object(
            self.headers
                .iter()
                .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
                .collect(),
        )
    }
}

/// An HTTP client bound to one base URL, recording everything it does.
#[derive(Clone)]
pub struct Client {
    base_url: String,
    agent: ureq::Agent,
    log: Arc<Mutex<Vec<Exchange>>>,
}

impl Client {
    /// Build a client for a base URL, which should include the `/v1` prefix.
    pub fn new(base_url: &str) -> Self {
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(std::time::Duration::from_secs(10))
            .timeout(std::time::Duration::from_secs(45))
            .build();
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            agent,
            log: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// The base URL, without a trailing slash.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Everything recorded so far.
    pub fn traffic(&self) -> Vec<Exchange> {
        self.log.lock().map(|g| g.clone()).unwrap_or_default()
    }

    /// Forget recorded traffic, so one case's scan does not search another case's exchanges.
    pub fn clear_traffic(&self) {
        if let Ok(mut g) = self.log.lock() {
            g.clear();
        }
    }

    /// Perform one call. Transport failures are errors; HTTP statuses are not.
    pub fn call(
        &self,
        method: &str,
        path: &str,
        query: &BTreeMap<String, String>,
        headers: &BTreeMap<String, String>,
        body: Option<&serde_json::Value>,
        principal: &Principal,
    ) -> Result<Response, String> {
        let mut url = format!("{}{}", self.base_url, path);
        if !query.is_empty() {
            let encoded: Vec<String> = query
                .iter()
                .map(|(k, v)| format!("{}={}", encode(k), encode(v)))
                .collect();
            url.push('?');
            url.push_str(&encoded.join("&"));
        }

        let mut request = self.agent.request(method, &url);
        let mut sent_headers = BTreeMap::new();

        match (principal.kind, principal.token.as_deref()) {
            (PrincipalKind::None, _) => {}
            (PrincipalKind::Machine | PrincipalKind::HumanBearer, Some(token)) => {
                sent_headers.insert("Authorization".to_string(), format!("Bearer {token}"));
            }
            (PrincipalKind::HumanCookie, Some(token)) => {
                sent_headers.insert("Cookie".to_string(), format!("handoff_session={token}"));
            }
            (kind, None) => {
                return Err(format!(
                    "principal is declared `{kind:?}` but carries no token in the profile"
                ))
            }
        }
        for (k, v) in headers {
            sent_headers.insert(k.clone(), v.clone());
        }
        let serialized = body.map(|b| b.to_string());
        if serialized.is_some() {
            sent_headers.insert("Content-Type".to_string(), "application/json".to_string());
        }
        sent_headers.insert("Accept".to_string(), "application/json".to_string());
        for (k, v) in &sent_headers {
            request = request.set(k, v);
        }

        let result = match &serialized {
            Some(payload) => request.send_string(payload),
            None => request.call(),
        };

        let (status, response_headers, response_body) = match result {
            Ok(resp) | Err(ureq::Error::Status(_, resp)) => {
                let status = resp.status();
                let names = resp.headers_names();
                let mut hs = BTreeMap::new();
                for name in names {
                    if let Some(v) = resp.header(&name) {
                        hs.insert(name.to_lowercase(), v.to_string());
                    }
                }
                let body = resp.into_string().unwrap_or_default();
                (status, hs, body)
            }
            Err(ureq::Error::Transport(t)) => {
                return Err(format!("{method} {url}: transport failure: {t}"))
            }
        };

        if let Ok(mut log) = self.log.lock() {
            log.push(Exchange {
                url: url.clone(),
                method: method.to_string(),
                request_headers: sent_headers,
                request_body: serialized,
                status,
                response_headers: response_headers.clone(),
                response_body: response_body.clone(),
            });
        }

        Ok(Response {
            status,
            headers: response_headers,
            body: response_body,
        })
    }

    /// Open a request and drop the connection without reading the response, the way a client
    /// process dying mid-poll does (C-11).
    pub fn abandon(
        &self,
        path: &str,
        query: &BTreeMap<String, String>,
        principal: &Principal,
        hold: std::time::Duration,
    ) -> Result<(), String> {
        use std::io::Write;
        use std::net::TcpStream;

        let url = format!("{}{}", self.base_url, path);
        let (host, port, target) = split_url(&url)?;
        let encoded: Vec<String> = query
            .iter()
            .map(|(k, v)| format!("{}={}", encode(k), encode(v)))
            .collect();
        let target = if encoded.is_empty() {
            target
        } else {
            format!("{target}?{}", encoded.join("&"))
        };

        let mut stream = TcpStream::connect((host.as_str(), port))
            .map_err(|e| format!("cannot open a poll to {host}:{port}: {e}"))?;
        let auth = match (principal.kind, principal.token.as_deref()) {
            (PrincipalKind::None, _) => String::new(),
            (PrincipalKind::HumanCookie, Some(t)) => format!("Cookie: handoff_session={t}\r\n"),
            (_, Some(t)) => format!("Authorization: Bearer {t}\r\n"),
            (_, None) => String::new(),
        };
        let head = format!(
            "GET {target} HTTP/1.1\r\nHost: {host}\r\nAccept: application/json\r\n{auth}Connection: close\r\n\r\n"
        );
        stream
            .write_all(head.as_bytes())
            .map_err(|e| format!("cannot send the poll: {e}"))?;
        std::thread::sleep(hold);
        drop(stream);
        Ok(())
    }
}

fn split_url(url: &str) -> Result<(String, u16, String), String> {
    let rest = url
        .strip_prefix("http://")
        .ok_or_else(|| format!("only plain HTTP base URLs can be abandoned mid-poll: {url}"))?;
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) => (h.to_string(), p.parse::<u16>().unwrap_or(80)),
        None => (authority.to_string(), 80),
    };
    Ok((host, port, path.to_string()))
}

fn encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_encodes_waiter_refs() {
        assert_eq!(encode("run:0198f2a1"), "run%3A0198f2a1");
    }

    #[test]
    fn splits_a_base_url_into_host_port_and_path() {
        let (h, p, t) = split_url("http://127.0.0.1:8080/v1/waiters/x/signals").unwrap();
        assert_eq!(
            (h.as_str(), p, t.as_str()),
            ("127.0.0.1", 8080, "/v1/waiters/x/signals")
        );
    }

    #[test]
    fn a_body_that_is_not_json_reads_as_null_rather_than_panicking() {
        let r = Response {
            status: 502,
            headers: BTreeMap::new(),
            body: "<html>gateway</html>".into(),
        };
        assert!(r.json().is_null());
    }

    #[test]
    fn a_body_that_is_not_json_is_a_failure_for_a_step_that_asserts_on_members() {
        // `null` satisfies every assertion of the form "this is not there", so a gateway error page
        // silently passes a case built out of negatives. Reading it as a document says so.
        let r = Response {
            status: 502,
            headers: BTreeMap::new(),
            body: "<html>gateway</html>".into(),
        };
        let err = r.document().unwrap_err();
        assert!(err.contains("not JSON"), "{err}");
        assert!(err.contains("<html>gateway</html>"), "{err}");

        // A 204 has no body and no members to assert against, and that is not an error.
        let empty = Response {
            status: 204,
            headers: BTreeMap::new(),
            body: String::new(),
        };
        assert!(empty.document().unwrap().is_null());
    }
}
