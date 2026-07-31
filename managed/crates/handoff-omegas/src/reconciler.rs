//! The derived read model: receipts in, summaries and meter readings out.
//!
//! This is the other half of the §3.1.2 reconciliation. [`crate::events`] argues *why* the mirror is
//! derived; this builds it, and it does so **without a single hook in the open core's write path**.
//!
//! That property is worth stating plainly, because it is the design's main claim. A hook — "on
//! answer, also call the meter" — would put a remote system inside the transaction that settles a
//! request, which is the one transaction that must not acquire new ways to fail. Instead the
//! reconciler *reads* what the open core already wrote, in its own time, and queues the derived
//! rows. The open server does not know this exists. Delete the managed deployment and the receipts
//! are unchanged, which is the test of whether a mirror is really a mirror.
//!
//! # The cursor is advanced last, on purpose
//!
//! Rows are queued, and only then does the cursor move. A crash between the two re-reads receipts it
//! has already summarized — and that is harmless, because the outbox is idempotent on
//! `(destination, dedupe_key)` and the dedupe key is derived from the receipt id. The other ordering
//! would skip.
//!
//! # A known cost, named rather than hidden
//!
//! [`handoff_core::ports::Store::chain`] returns a tenant's **whole** chain, because it exists to
//! serve the open verifier, which has to walk all of it. Reconciling from it is therefore O(chain)
//! per pass rather than O(new). It is correct and it will not stay acceptable: the upstream
//! follow-up is a cursored read on the open store, which is a port change and belongs in the open
//! repo. Until then this is a real cost on a large tenant and it should not be discovered in
//! production.

use handoff_core::ports::{BoxFuture, Store};
use handoff_protocol::error::Result;
use handoff_protocol::receipt::Receipt;
use std::sync::Arc;

use crate::events::summarize;
use crate::meter::{self, Metered};
use crate::outbox::{Destination, Outbox};

/// Where the reconciler reads receipts from.
pub trait ReceiptSource: Send + Sync {
    /// Every receipt in this tenant's chain after `after`, oldest first.
    ///
    /// `after` is a receipt id, not an index: an index would break the moment a chain is exported
    /// and re-imported, and the id is what the cursor stores anyway.
    fn receipts_since(
        &self,
        tenant: String,
        after: Option<String>,
    ) -> BoxFuture<'_, Result<Vec<Receipt>>>;
}

/// The real source: the open core's own chain export.
pub struct StoreReceipts<S: Store> {
    store: Arc<S>,
}

impl<S: Store> StoreReceipts<S> {
    /// Read from this store.
    pub fn new(store: Arc<S>) -> Self {
        Self { store }
    }
}

impl<S: Store + 'static> ReceiptSource for StoreReceipts<S> {
    fn receipts_since(
        &self,
        tenant: String,
        after: Option<String>,
    ) -> BoxFuture<'_, Result<Vec<Receipt>>> {
        Box::pin(async move {
            let export = self.store.chain(tenant).await?;
            Ok(after_receipt(export.receipts, after.as_deref()))
        })
    }
}

/// Everything strictly after `after`, or everything when the cursor has never moved.
///
/// A cursor naming a receipt that is no longer in the chain returns **nothing** rather than
/// everything. Re-mirroring a tenant's entire history because one id went missing would be a far
/// worse failure than a stalled cursor an operator can see and reset.
fn after_receipt(receipts: Vec<Receipt>, after: Option<&str>) -> Vec<Receipt> {
    let Some(after) = after else {
        return receipts;
    };
    match receipts.iter().position(|r| r.id.to_string() == after) {
        Some(index) => receipts.into_iter().skip(index + 1).collect(),
        None => Vec::new(),
    }
}

/// Turns receipts into queued rows.
pub struct Reconciler {
    source: Box<dyn ReceiptSource>,
    outbox: Arc<Outbox>,
}

/// What one pass did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Pass {
    /// Receipts read.
    pub receipts: usize,
    /// Audit summaries queued. Fewer than `receipts` when a pass re-reads.
    pub events_queued: usize,
    /// Meter readings queued.
    pub readings_queued: usize,
}

impl Reconciler {
    /// Build one.
    pub fn new(source: Box<dyn ReceiptSource>, outbox: Arc<Outbox>) -> Self {
        Self { source, outbox }
    }

    /// Reconcile one tenant.
    ///
    /// Tenant *discovery* is deliberately not here. The open store has no "list every tenant"
    /// method — it has never needed one — and inventing a query against its tables from this crate
    /// would couple the managed adapter to the open core's schema, which is exactly the coupling the
    /// whole arrangement exists to avoid. The deployment drives this per tenant from the set it
    /// already knows, and `Store::tenants()` is a named upstream follow-up.
    pub async fn run_for(&self, tenant: &str) -> Result<Pass> {
        let cursor = self.outbox.cursor(tenant).await?;
        let receipts = self
            .source
            .receipts_since(tenant.to_string(), cursor)
            .await?;

        let mut pass = Pass {
            receipts: receipts.len(),
            ..Pass::default()
        };
        let mut last: Option<String> = None;

        for receipt in &receipts {
            let summary = summarize(receipt)?;
            let occurred_at = receipt.decided_at.to_datetime();

            if self
                .outbox
                .enqueue(
                    Destination::Event,
                    tenant,
                    &format!("{}:{}", crate::events::RECEIPT_RECORDED, receipt.id),
                    serde_json::to_value(&summary).map_err(serialization)?,
                    occurred_at,
                )
                .await?
            {
                pass.events_queued += 1;
            }

            // A receipt is one intervention. Requests, deliveries and callbacks are counted where
            // they happen; this pass is the one that can only be derived from an outcome.
            let reading = meter::reading(
                tenant,
                &receipt.request_id,
                Metered::Intervention,
                receipt.decided_at,
            )?;
            if self
                .outbox
                .enqueue(
                    Destination::Meter,
                    tenant,
                    &reading.idempotency_key,
                    serde_json::to_value(&reading).map_err(serialization)?,
                    occurred_at,
                )
                .await?
            {
                pass.readings_queued += 1;
            }

            last = Some(receipt.id.to_string());
        }

        // Last, and only after every row for every receipt is durable. A crash before this point
        // re-reads; a crash after it would have skipped.
        if let Some(last) = last {
            self.outbox.advance(tenant, &last).await?;
        }
        Ok(pass)
    }
}

fn serialization(e: serde_json::Error) -> handoff_protocol::error::ProtocolError {
    handoff_protocol::error::ProtocolError::new(
        handoff_protocol::error::ErrorCode::InvalidRequest,
        format!("a derived row could not be serialized: {e}"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures;

    fn chain_of(n: usize) -> Vec<Receipt> {
        fixtures::chain(n)
    }

    #[test]
    fn a_cursor_that_has_never_moved_reads_everything() {
        assert_eq!(after_receipt(chain_of(3), None).len(), 3);
    }

    #[test]
    fn a_cursor_reads_strictly_after_itself() {
        let chain = chain_of(3);
        let first = chain[0].id.to_string();
        let last = chain[2].id.to_string();
        assert_eq!(after_receipt(chain.clone(), Some(&first)).len(), 2);
        assert_eq!(after_receipt(chain, Some(&last)).len(), 0);
    }

    #[test]
    fn a_cursor_naming_a_receipt_that_is_gone_reads_nothing_rather_than_everything() {
        // Re-mirroring a tenant's whole history because one id went missing is a far worse failure
        // than a stalled cursor an operator can see.
        assert_eq!(
            after_receipt(chain_of(3), Some("rcpt_01K3M7QW8ZC4YRXB2N6VD9FTHZ")).len(),
            0
        );
    }
}
