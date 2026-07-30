//! `ReceiptSigner` — the attestation that does not exist yet, and refuses to pretend.
//!
//! Independent attestation is the strongest thing on the hosted tier's list, and it is the one item
//! on that list where **the hosted tier does not deliver it either**. There is no Ωmegas signing
//! key, no key custody, no public verification endpoint, and no service behind any of it. The
//! discovery is blunt about this: *"the managed attestation service does not exist yet either. Do
//! not market it before it does."*
//!
//! So this adapter refuses, loudly, and the refusal is the deliverable. A stub that returned a
//! signature from some convenient key would be worse than nothing: it would produce receipts that
//! *look* attested, and the whole value of attestation is that a receipt cannot be produced by the
//! party it is evidence against. A fake attestation is not a partial feature. It is a false claim
//! with a signature on it.
//!
//! # Why it is structural, not withheld
//!
//! A self-hosted receipt is signed with a key the operator controls, so the operator can produce any
//! receipt they wish. That is adequate for internal control and worthless as evidence *against* the
//! operator. Only a party that is not the operator can attest — which is the definition of a third
//! party, not a feature anyone chose to withhold.
//!
//! # Two things must land before this becomes real
//!
//! 1. **The key and its custody.** An attestation key held by the same team that runs the service is
//!    a weaker claim than it appears, and where it lives is a decision nobody has made.
//! 2. **A port on the open core.** The receipt is sealed inside the answer transaction, and nothing
//!    in [`handoff_core::ports::Store`] takes a signer. Wiring attestation means the open server
//!    gains a `ReceiptSigner` it consults while sealing — an upstream change, in the open repo,
//!    visible to everyone. It must **not** be a managed-only step that produces receipts the open
//!    verifier cannot check, because a receipt only its issuer can verify is a vendor claim rather
//!    than evidence.
//!
//! The verifier stays open and stays a pure function, whatever happens here.

use handoff_core::ports::BoxFuture;
use handoff_core::seam::{Attestation, ReceiptSigner};
use handoff_protocol::error::Result;
use handoff_protocol::id::ReceiptId;
use handoff_protocol::receipt::Digest;

use crate::dependency::MissingDependency;

/// The attestation service, as far as it exists.
#[derive(Debug, Clone, Copy, Default)]
pub struct OmegasAttestor;

impl ReceiptSigner for OmegasAttestor {
    fn attest(
        &self,
        _tenant: String,
        _receipt_id: ReceiptId,
        _digest: Digest,
    ) -> BoxFuture<'_, Result<Attestation>> {
        Box::pin(async { Err(MissingDependency::ATTESTATION_KEY.into_error()) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn attestation_refuses_rather_than_signing_with_whatever_key_is_lying_around() {
        let error = OmegasAttestor
            .attest(
                "org_01K3M7QW8ZC4YRXB2N6VD9FTHE".into(),
                ReceiptId::parse("rcpt_01K3M7QW8ZC4YRXB2N6VD9FTHE").expect("parse"),
                Digest::sha256(b"anything"),
            )
            .await
            .expect_err("there is no attestation key");
        assert!(error
            .message
            .contains("attest a receipt as a party other than the operator"));
        assert!(error.message.contains("does not exist yet"));
    }

    #[tokio::test]
    async fn the_refusal_names_no_milestone_because_nothing_owns_this_yet() {
        // Naming a milestone that does not own it would put this on someone's plan by accident.
        let error = OmegasAttestor
            .attest(
                "org_01K3M7QW8ZC4YRXB2N6VD9FTHE".into(),
                ReceiptId::parse("rcpt_01K3M7QW8ZC4YRXB2N6VD9FTHE").expect("parse"),
                Digest::sha256(b"anything"),
            )
            .await
            .expect_err("no key");
        assert!(error.message.contains("no milestone owns it"));
    }
}
