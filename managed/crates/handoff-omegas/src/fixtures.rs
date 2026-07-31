//! Test fixtures shared across this crate's suites.
//!
//! Every value here is built through the open core's own types and sealed through its own
//! [`Receipt::seal`], rather than hand-written JSON. A fixture that bypasses the type it stands for
//! is a fixture that keeps passing after the type changes underneath it.

use handoff_core::seam::{ContactPoint, Envelope, Recipient};
use handoff_protocol::clock::Timestamp;
use handoff_protocol::id::{DeliveryId, OrgId, PrincipalId, ReceiptId, RequestId};
use handoff_protocol::receipt::{
    ActorType, Digest, Receipt, ReceiptActor, ReceiptAuthority, ReceiptDecision, ReceiptKind,
    ReceiptVia, SatisfiedStrength,
};
use handoff_protocol::request::{Disposition, Prompt};
use handoff_protocol::requires::{AuthStrength, Authority};

/// The tenant every fixture belongs to.
pub const ORG: &str = "org_01K3M7QW8ZC4YRXB2N6VD9FTHE";

fn timestamp(millis: i64) -> Timestamp {
    Timestamp::from_millis(millis).expect("a representable instant")
}

fn id_at(prefix: &str, n: usize) -> String {
    // ULIDs are Crockford base32; varying the final character keeps them well-formed and distinct.
    let alphabet = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    let last = alphabet[n % alphabet.len()] as char;
    format!("{prefix}_01K3M7QW8ZC4YRXB2N6VD9FTH{last}")
}

/// One receipt for a person's decision, unsealed.
pub fn decision_receipt(n: usize) -> Receipt {
    Receipt {
        id: ReceiptId::parse(&id_at("rcpt", n)).expect("receipt id"),
        request_id: RequestId::parse(&id_at("req", n)).expect("request id"),
        org_id: OrgId::parse(ORG).expect("org id"),
        kind: ReceiptKind::Decision,
        corrects: None,
        decision: ReceiptDecision {
            // A real answer, so a test asserting that the mirror does not carry content is
            // asserting something.
            values: serde_json::json!({"approve": true, "note_to_finance": "ship it"})
                .as_object()
                .expect("object")
                .clone(),
            disposition: Disposition::Decide,
            note: Some("looks right to me".into()),
        },
        actor: ReceiptActor {
            actor_type: ActorType::User,
            principal_id: Some(
                PrincipalId::parse("usr_01K3M7QW8ZC4YRXB2N6VD9FTHE").expect("principal"),
            ),
            display: Some("Dana".into()),
            role_at_decision: Some("admin".into()),
            auth_strength: Some(AuthStrength::Session),
            reauth_at: None,
            mfa_at: None,
            on_behalf_of: None,
            ip_digest: None,
            user_agent_digest: None,
        },
        decided_at: timestamp(1_700_000_000_000 + (n as i64) * 1_000),
        attempt_id: None,
        request_version: 1,
        request_digest: Digest::sha256(format!("request-{n}").as_bytes()),
        rendered: None,
        via: ReceiptVia::default(),
        authority: ReceiptAuthority {
            required: Authority::default(),
            satisfied: SatisfiedStrength::Session,
        },
        steps: Vec::new(),
        capabilities_exercised: Vec::new(),
        clearance: None,
        chain: None,
        presentation_divergence: None,
    }
}

/// One sealed receipt, first in its chain.
pub fn sealed_receipt() -> Receipt {
    decision_receipt(0).seal(None).expect("seal")
}

/// A sealed chain of `n` receipts, oldest first.
pub fn chain(n: usize) -> Vec<Receipt> {
    let mut sealed: Vec<Receipt> = Vec::with_capacity(n);
    for i in 0..n {
        let previous = sealed.last().and_then(|r| r.chain.clone());
        sealed.push(
            decision_receipt(i)
                .seal(previous.as_ref())
                .expect("seal into the chain"),
        );
    }
    sealed
}

/// One delivery envelope.
pub fn envelope(channel: &str) -> Envelope {
    Envelope {
        tenant: ORG.into(),
        request_id: RequestId::parse("req_01K3M7QW8ZC4YRXB2N6VD9FTHE").expect("request id"),
        delivery_id: DeliveryId::parse("dlv_01K3M7QW8ZC4YRXB2N6VD9FTHE").expect("delivery id"),
        channel: channel.into(),
        recipient: Recipient {
            principal_id: Some(
                PrincipalId::parse("usr_01K3M7QW8ZC4YRXB2N6VD9FTHE").expect("principal"),
            ),
            display: Some("Dana".into()),
            timezone: Some("Europe/Berlin".into()),
            contacts: vec![ContactPoint {
                channel: channel.into(),
                address: "dana@example.test".into(),
                verified: true,
            }],
            quiet_hours: None,
        },
        prompt: Prompt {
            title: "Approve the refund".into(),
            body: None,
            evidence: Vec::new(),
        },
        surface_url: "https://handoff.omegas.dev/r/req_01K3M7QW8ZC4YRXB2N6VD9FTHE".into(),
        rung: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fixture_chain_verifies_against_the_open_verifier() {
        // If the fixtures did not verify, every test asserting something about a receipt would be
        // asserting it about a shape the protocol would reject.
        let sealed = chain(4);
        let head = handoff_protocol::receipt::verify_chain(&sealed, timestamp(1_800_000_000_000))
            .expect("verify")
            .expect("a head");
        assert_eq!(head.height, 4);
        assert_eq!(head.org_id.to_string(), ORG);
    }

    #[test]
    fn each_receipt_in_a_fixture_chain_has_its_own_identity() {
        let sealed = chain(3);
        let mut ids: Vec<String> = sealed.iter().map(|r| r.id.to_string()).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), 3);
    }
}
