//! Error reporting, off unless a deployment asks for it.
//!
//! This module exists behind the `sentry` feature and is **not built by default**. README states
//! that a self-hosted deployment has "no dependency on any hosted service, no phone-home, no licence
//! key", and that has to stay literally true: without the feature the crate does not link an error
//! reporter at all, and with the feature but no `SENTRY_DSN` it initialises nothing and opens no
//! socket. Our own deployment turns it on. A fork gets a binary that cannot report anywhere.
//!
//! # What may leave this process
//!
//! The protocol's whole subject is authorization, so an error reporter is a data-egress path
//! pointed at a third party, and the interesting question is not what it strips but what it sends.
//! `scrub` therefore **rebuilds** each event from an allowlist rather than deleting fields from it:
//! anything not named here cannot reach the wire, including fields added by a future SDK version
//! that nobody re-reviewed. A deny-list would have to be right about every field that exists now
//! and every field that will exist later; an allowlist only has to be right about what we need.
//!
//! What is sent: the exception type, a redacted message, stack frames reduced to file, function and
//! line, the level, the release, and two tags. What is dropped: the request (URL, method, headers,
//! body), the user, breadcrumbs, `extra`, contexts, server name, and the module list.
//!
//! Even the message is not trusted. Error strings interpolate values -- `ProtocolError` messages
//! quote the offending field, and a store error can carry a fragment of a statement -- so every
//! string that survives is passed through `redact`, which removes protocol identifiers and
//! credential shapes wherever they appear.

use std::borrow::Cow;

/// Live for as long as the process should report errors. Dropping it flushes.
pub struct Reporter {
    _guard: sentry::ClientInitGuard,
}

/// Start reporting, if this deployment has asked for it.
///
/// Returns `None` when `SENTRY_DSN` is unset, which is the default and the self-hosted case. No DSN
/// means no client, no transport thread, and no outbound connection.
pub fn init() -> Option<Reporter> {
    let dsn = std::env::var("SENTRY_DSN")
        .ok()
        .filter(|d| !d.trim().is_empty())?;

    let guard = sentry::init((
        dsn,
        sentry::ClientOptions {
            // `handoff@<core version>`, not the app or machine name. This project is shared with
            // Omega Agent permanently, so the release has to say which product a regression belongs
            // to on its own, without anyone cross-referencing a deployment.
            release: Some(Cow::Owned(format!("handoff@{}", env!("CARGO_PKG_VERSION")))),
            environment: Some(Cow::Owned(
                std::env::var("HANDOFF_ENVIRONMENT").unwrap_or_else(|_| "production".to_string()),
            )),
            // The default is already false. It is set explicitly because this is the switch that
            // would attach headers, cookies and bodies to events, and a default is a thing someone
            // changes without reading this file.
            send_default_pii: false,
            // No performance traces. They carry URLs, and a URL here carries identifiers.
            traces_sample_rate: 0.0,
            // Breadcrumbs accumulate log records, and this server logs what it is asked to do.
            max_breadcrumbs: 0,
            attach_stacktrace: true,
            before_send: Some(std::sync::Arc::new(|event| Some(scrub(event)))),
            // Same treatment for the breadcrumb path, in case max_breadcrumbs is ever raised.
            before_breadcrumb: Some(std::sync::Arc::new(|_| None)),
            ..Default::default()
        },
    ));

    sentry::configure_scope(|scope| {
        // `service` is the only thing separating Handoff from the Omega Agent events sharing this
        // project, and that arrangement is permanent rather than interim. It is deliberately the
        // PRODUCT name and not the Fly app name: `handoff-v1` would break continuity the day a
        // `handoff-v2` app exists, and an alert rule scoped to a name that moves is an alert rule
        // that silently stops matching.
        scope.set_tag("service", "handoff");
        scope.set_tag("component", "handoffd");
    });

    Some(Reporter { _guard: guard })
}

/// A `tracing` layer that turns ERROR-level records into Sentry events.
///
/// Wired because 5xx alone does not cover the failure this deployment was instrumented for. A store
/// that will not answer is reported as `invalid_request`, an HTTP **400** -- the specification
/// classifies errors by what the caller should do, not by whose fault it is, which is right for the
/// protocol and useless for alerting. The database going down would have produced a stream of 400s
/// and complete silence in Sentry.
///
/// `handoff-store-postgres` already emits `tracing::error!` for exactly those failures, so the
/// signal exists; this carries it. Field values are NOT carried: `scrub` drops `extra`, which is
/// where the tracing bridge puts them, so what arrives is the record's message and its level.
#[must_use]
pub fn tracing_layer<S>() -> sentry_tracing::SentryLayer<S>
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    sentry_tracing::layer().event_filter(|metadata| match *metadata.level() {
        // ERROR becomes an event. WARN and below are dropped entirely rather than kept as
        // breadcrumbs: this server logs what it is asked to do, and a breadcrumb trail of that is a
        // transcript of tenant activity sitting in a third party.
        tracing::Level::ERROR => sentry_tracing::EventFilter::Event,
        _ => sentry_tracing::EventFilter::Ignore,
    })
}

/// Rebuild an event from the fields we have decided may leave, dropping everything else.
fn scrub(event: sentry::protocol::Event<'static>) -> sentry::protocol::Event<'static> {
    let mut out = sentry::protocol::Event {
        event_id: event.event_id,
        timestamp: event.timestamp,
        level: event.level,
        platform: event.platform,
        release: event.release,
        environment: event.environment,
        logger: event.logger,
        sdk: event.sdk,
        ..Default::default()
    };

    // Namespaced grouping, set here rather than at a call site so it cannot be forgotten at one.
    // Sentry groups `capture_message` events by message text, and this project now carries two
    // products permanently: "the store rejected this operation" is a plausible sentence in either
    // codebase, and without a service-specific first element the two would land in one issue and
    // page the wrong team. `{{ default }}` keeps Sentry's normal grouping WITHIN Handoff.
    out.fingerprint = vec![Cow::Borrowed("handoff"), Cow::Borrowed("{{ default }}")].into();

    // Only the two tags this process sets. A tag added at a call site is not on the allowlist,
    // because a call site is exactly where a value gets attached without anyone reviewing it.
    for key in ["service", "component"] {
        if let Some(value) = event.tags.get(key) {
            out.tags.insert(key.to_string(), redact(value));
        }
    }

    out.message = event.message.as_deref().map(redact);

    out.exception = event
        .exception
        .into_iter()
        .map(|mut exception| {
            exception.value = exception.value.as_deref().map(redact);
            exception.stacktrace = exception.stacktrace.map(|mut stacktrace| {
                for frame in &mut stacktrace.frames {
                    // Frames are kept for their location and nothing else. `vars` would carry local
                    // variables, which in this codebase means answer values and credentials.
                    frame.vars.clear();
                    frame.pre_context.clear();
                    frame.post_context.clear();
                    frame.context_line = None;
                    frame.function = frame.function.as_deref().map(redact);
                    frame.filename = frame.filename.as_deref().map(redact);
                }
                stacktrace
            });
            exception
        })
        .collect();

    out
}

/// Remove anything shaped like a protocol identifier or a credential.
///
/// Uniform by shape rather than by name: every identifier this protocol mints is a lowercase prefix
/// and a 26-character Crockford ULID (§1.4), so one rule covers `usr_`, `sa_`, `org_`, `hg_` and
/// every prefix added later. Credentials do not share that shape and are listed separately.
///
/// `SECURITY.md` asks for request identifiers to be treated as sensitive too, so they are redacted
/// with the rest rather than kept for convenience. What survives is the error's shape and where it
/// happened, which is what a stack trace is for.
fn redact(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        if let Some(len) = credential_at(&input[i..]) {
            out.push_str("<redacted>");
            i += len;
        } else if let Some(len) = identifier_at(&input[i..]) {
            out.push_str("<id>");
            i += len;
        } else {
            let ch = input[i..].chars().next().unwrap_or('\u{fffd}');
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    out
}

/// A bearer credential: opaque, high entropy, and never useful in a report.
fn credential_at(rest: &str) -> Option<usize> {
    const PREFIXES: [&str; 6] = ["omg_", "hs_", "lnk_", "whsec_", "rt_", "sk-"];
    for prefix in PREFIXES {
        if let Some(tail) = rest.strip_prefix(prefix) {
            let run = tail
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
                .count();
            // Long enough to be a secret rather than a word that happens to start this way.
            if run >= 12 {
                return Some(prefix.len() + run);
            }
        }
    }
    // `Authorization: Bearer …` in any casing.
    let lower = rest.to_ascii_lowercase();
    if let Some(tail) = lower.strip_prefix("bearer ") {
        let run = tail
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-' || *c == '.')
            .count();
        if run > 0 {
            return Some("bearer ".len() + run);
        }
    }
    None
}

/// `<lowercase prefix>_<26 Crockford base32 characters>`, the shape of every protocol identifier.
fn identifier_at(rest: &str) -> Option<usize> {
    let underscore = rest.find('_')?;
    if underscore == 0 || underscore > 8 {
        return None;
    }
    if !rest[..underscore].chars().all(|c| c.is_ascii_lowercase()) {
        return None;
    }
    let tail = &rest[underscore + 1..];
    let ulid: String = tail.chars().take(26).collect();
    if ulid.len() == 26 && ulid.chars().all(|c| c.is_ascii_alphanumeric()) {
        // Not followed by more identifier characters, or it is something longer that merely starts
        // this way.
        let next = tail.chars().nth(26);
        if next.is_none_or(|c| !c.is_ascii_alphanumeric()) {
            return Some(underscore + 1 + 26);
        }
    }
    None
}

/// Report a server-side failure. Called only where a 5xx is produced.
pub fn capture_server_error(error: &handoff_protocol::error::ProtocolError) {
    sentry::with_scope(
        |scope| {
            // The code is a closed enumeration in this repository, so it is safe to send and is the
            // one thing that makes an event triageable at a glance.
            scope.set_tag("error_code", error.code.as_str());
        },
        || {
            sentry::capture_message(
                &redact(&format!("{}: {}", error.code.as_str(), error.message)),
                sentry::Level::Error,
            );
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credentials_and_identifiers_do_not_survive_redaction() {
        let cases = [
            ("token omg_handoff_test_ka_conformance failed", "omg_"),
            (
                "secret whsec_2f8a91c4e7b3d05a6c1e9f47b28d3a05 rotated",
                "whsec_",
            ),
            ("Authorization: Bearer abc123def456ghi789", "earer abc"),
            ("session hs_editor_one_conformance expired", "hs_"),
            ("request req_01K3M7QW8ZC4YRXB2N6VD9FTHA missing", "req_01"),
            ("principal usr_01K3M7QW8ZC4YRXB2N6VD9FTH3 denied", "usr_01"),
            ("grant hg_01K3M7QW8ZC4YRXB2N6VD9FTHB revoked", "hg_01"),
            ("tenant org_01K3M7QW8ZC4YRXB2N6VD9FTHA", "org_01"),
        ];
        for (input, must_not_appear) in cases {
            let redacted = redact(input);
            assert!(
                !redacted.contains(must_not_appear),
                "`{must_not_appear}` survived redaction of `{input}` as `{redacted}`"
            );
        }
    }

    #[test]
    fn ordinary_words_are_left_alone() {
        // Redaction that eats the message is redaction nobody keeps. These must pass through.
        for text in [
            "the store rejected this operation",
            "a machine principal may never answer a human-intervention request",
            "expected height 1, found 0",
        ] {
            assert_eq!(redact(text), text, "redaction damaged an ordinary message");
        }
    }

    #[test]
    fn scrub_drops_everything_not_on_the_allowlist() {
        let mut event = sentry::protocol::Event {
            message: Some("request req_01K3M7QW8ZC4YRXB2N6VD9FTHA failed".to_string()),
            server_name: Some("secret-host".into()),
            ..Default::default()
        };
        event.tags.insert("service".into(), "handoff-v1".into());
        event.tags.insert("answer".into(), "approve".into());
        event.extra.insert("body".into(), "{\"values\":{}}".into());
        event.request = Some(sentry::protocol::Request {
            url: Some(
                "https://handoff.omegas.dev/v1/requests/req_01K3M7QW8ZC4YRXB2N6VD9FTHA"
                    .parse()
                    .unwrap(),
            ),
            ..Default::default()
        });
        event.user = Some(sentry::protocol::User {
            id: Some("usr_01K3M7QW8ZC4YRXB2N6VD9FTH3".into()),
            ..Default::default()
        });

        let out = scrub(event);

        assert!(out.request.is_none(), "the request survived the allowlist");
        assert!(out.user.is_none(), "the user survived the allowlist");
        assert!(out.extra.is_empty(), "extra survived the allowlist");
        assert!(
            out.server_name.is_none(),
            "the server name survived the allowlist"
        );
        assert!(out.breadcrumbs.is_empty());
        assert_eq!(
            out.tags.get("service").map(String::as_str),
            Some("handoff-v1")
        );
        assert!(!out.tags.contains_key("answer"), "an unlisted tag survived");
        let message = out.message.unwrap_or_default();
        assert!(
            !message.contains("req_01"),
            "an identifier survived: {message}"
        );
    }

    #[test]
    fn every_event_is_fingerprinted_into_its_own_product() {
        // omega-prod carries two products permanently, and Sentry groups capture_message events by
        // message text. Without this, a sentence both codebases could plausibly emit merges into one
        // issue and an incident in one product pages the other.
        let out = scrub(sentry::protocol::Event {
            message: Some("the store rejected this operation".to_string()),
            ..Default::default()
        });
        let fingerprint: Vec<String> = out.fingerprint.iter().map(|c| c.to_string()).collect();
        assert_eq!(fingerprint.first().map(String::as_str), Some("handoff"));
        assert!(fingerprint.contains(&"{{ default }}".to_string()));
    }
}
