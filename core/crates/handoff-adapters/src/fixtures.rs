//! One envelope, built the same way for every adapter's tests.
//!
//! Shared so that each adapter's test asserts on its *own* behaviour rather than on a subtly
//! different envelope it built itself. A per-adapter fixture is how two adapters end up disagreeing
//! about what a delivery looks like without either test noticing.

use handoff_core::seam::{ContactPoint, Envelope, Recipient};
use handoff_protocol::request::Prompt;

/// A person the directory knows, reachable on `channel` at `address` when one is given.
pub(crate) fn recipient(channel: &str, address: Option<&str>) -> Recipient {
    Recipient {
        principal_id: handoff_protocol::id::PrincipalId::parse("usr_01K3M7QW8ZC4YRXB2N6VD9FTHE")
            .ok(),
        display: Some("Dana".into()),
        timezone: None,
        contacts: address
            .map(|address| ContactPoint {
                channel: channel.to_string(),
                address: address.to_string(),
                verified: true,
            })
            .into_iter()
            .collect(),
        quiet_hours: None,
    }
}

/// One delivery on `channel`, addressed to `recipient`.
pub(crate) fn envelope(channel: &str, recipient: Recipient) -> Envelope {
    Envelope {
        tenant: "org_01K3M7QW8ZC4YRXB2N6VD9FTHA".into(),
        request_id: handoff_protocol::id::RequestId::parse("req_01K3M7QW8ZC4YRXB2N6VD9FTHE")
            .expect("a request id"),
        delivery_id: handoff_protocol::id::DeliveryId::parse("dlv_01K3MB2R6C8ZC4YRXB2N6VD9FT")
            .expect("a delivery id"),
        channel: channel.to_string(),
        recipient,
        prompt: Prompt {
            title: "Approve the release?".into(),
            body: None,
            evidence: Vec::new(),
        },
        surface_url: "https://example.test/requests/req_01K3M7QW8ZC4YRXB2N6VD9FTHE".into(),
        rung: 0,
    }
}
