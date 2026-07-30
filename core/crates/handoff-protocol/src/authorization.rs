//! AUTHORIZATION (§10): what the runtime **spends**.
//!
//! The receipt records what was decided; the authorization is the thing that makes the decision
//! usable exactly once. Three properties carry the whole design:
//!
//! * **One answer mints exactly one authorization** (I10).
//! * **Redemption is idempotent per `effect_key`.** A retried agent turn cannot double-spend, and a
//!   single-use authorization presented with a *different* effect key is `409 authorization_spent`.
//! * **`effect_digest` binds the authorization to the shape of the effect.** An approval of
//!   "refund $2,400" cannot be spent on "refund $24,000".
//!
//! `advisory` and `gated` are modes of one design (§10.1), and nothing in this module takes a mode
//! parameter. The mode decides whether the runtime *must* redeem before acting; it never changes
//! what redemption does. A Server that forked its state machine between the two would have built
//! the interception gate §10.1 explicitly rules out.

use crate::clock::Timestamp;
use crate::error::{ErrorCode, ProtocolError, Result};
use crate::id::{AuthorizationId, ReceiptId, RequestId};
use crate::receipt::Digest;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// The RECOMMENDED window a decision stays spendable (§10 rule 4).
///
/// An approval is a decision about a moment; spending it days later is a different act, and the
/// protocol says so instead of hoping.
pub const DEFAULT_AUTHORIZATION_TTL_SECS: u64 = 24 * 60 * 60;

/// What an authorization is tied to (§10).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthorizationBinding {
    /// The unit of runtime work this decision belongs to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub waiter_ref: Option<String>,
    /// Digest of the effect's parameters.
    ///
    /// When present, a redemption whose digest disagrees is refused. This is the difference between
    /// authorizing *an action* and authorizing *this* action.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effect_digest: Option<Digest>,
}

/// One spend, keyed by the caller's own effect key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Redemption {
    /// The caller's stable identifier for the effect.
    pub effect_key: String,
    /// When it was spent, on the Server's clock.
    pub redeemed_at: Timestamp,
}

/// Whether an authorization can still be spent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthorizationState {
    /// Spendable.
    Open,
    /// Already spent, and single-use.
    Spent,
    /// Past its `expires_at`.
    Expired,
}

/// Spend the authorization against exactly one effect (§10).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RedeemRequest {
    /// A stable identifier for the effect this decision authorizes — **the same string on every
    /// retry of the same effect**. Choosing a key that varies per attempt defeats the entire
    /// mechanism, so it is the caller's job to make it stable.
    pub effect_key: String,
    /// Digest of the effect's parameters, compared against the binding when one is set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effect_digest: Option<Digest>,
}

/// The whole answer a caller needs: `first_redemption: true` means act, `false` means this effect
/// already happened and must not happen again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RedeemResult {
    /// When the spend was recorded. On a replay this is the *original* time, not now.
    pub redeemed_at: Timestamp,
    /// Whether this call is the one that recorded the spend.
    pub first_redemption: bool,
}

/// What the runtime spends (§10).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Authorization {
    /// This authorization.
    pub id: AuthorizationId,
    /// The receipt it was minted with. Exactly one authorization per answer (I10).
    pub receipt_id: ReceiptId,
    /// The request that was answered.
    pub request_id: RequestId,
    /// The decided values this authorization carries — what was actually approved.
    #[serde(default)]
    pub grants: Map<String, Value>,
    /// When true, a second **different** `effect_key` is refused.
    pub single_use: bool,
    /// When it stops being spendable. `None` means it never lapses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<Timestamp>,
    /// What this authorization is tied to.
    #[serde(default)]
    pub bound_to: AuthorizationBinding,
    /// Every spend so far.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub redemptions: Vec<Redemption>,
}

impl Authorization {
    /// Mint the single authorization an answer produces (I10).
    ///
    /// Defaults to single-use, because the protocol's third guarantee is that one answer authorizes
    /// exactly one effect (§1.3).
    pub fn mint(
        id: AuthorizationId,
        receipt_id: ReceiptId,
        request_id: RequestId,
        grants: Map<String, Value>,
    ) -> Self {
        Self {
            id,
            receipt_id,
            request_id,
            grants,
            single_use: true,
            expires_at: None,
            bound_to: AuthorizationBinding::default(),
            redemptions: Vec::new(),
        }
    }

    /// Bind this authorization to a unit of runtime work and, optionally, to the shape of the
    /// effect.
    #[must_use]
    pub fn bound_to(mut self, binding: AuthorizationBinding) -> Self {
        self.bound_to = binding;
        self
    }

    /// Set when this authorization stops being spendable.
    #[must_use]
    pub fn expiring_at(mut self, expires_at: Timestamp) -> Self {
        self.expires_at = Some(expires_at);
        self
    }

    /// Whether this authorization is open, spent, or expired at `now`.
    pub fn state_at(&self, now: Timestamp) -> AuthorizationState {
        if self.expires_at.is_some_and(|at| at.is_at_or_before(now)) {
            AuthorizationState::Expired
        } else if self.single_use && !self.redemptions.is_empty() {
            AuthorizationState::Spent
        } else {
            AuthorizationState::Open
        }
    }

    /// Attempt a redemption.
    ///
    /// Pure: on a first redemption the returned [`Redemption`] is what the caller must persist
    /// **atomically with the check**. Persisting it separately reintroduces the double-spend this
    /// method exists to prevent, in exactly the way a read-then-write reintroduces the double
    /// answer of §6.7.
    ///
    /// The checks run in this order, and the order is deliberate: expiry and the effect binding are
    /// evaluated before the replay check, so a stale or mis-shaped redemption is refused rather
    /// than quietly matching a previous spend.
    pub fn redeem(
        &self,
        request: &RedeemRequest,
        now: Timestamp,
    ) -> Result<(RedeemResult, Option<Redemption>)> {
        if request.effect_key.is_empty() || request.effect_key.len() > 256 {
            return Err(ProtocolError::new(
                ErrorCode::InvalidRequest,
                "`effect_key` must be 1..=256 bytes",
            ));
        }

        if self.expires_at.is_some_and(|at| at.is_at_or_before(now)) {
            // The taxonomy has no `authorization_expired`; see the crate documentation's spec
            // defect D-4. Overloading `authorization_spent` would tell the caller something untrue.
            return Err(ProtocolError::new(
                ErrorCode::InvalidRequest,
                "this authorization is past its expiry and is no longer spendable",
            ));
        }

        // The effect binding, before anything else about spending. An approval of one amount must
        // never be spendable on another, whatever the effect key says.
        if let Some(bound) = &self.bound_to.effect_digest {
            match &request.effect_digest {
                Some(offered) if offered == bound => {}
                _ => {
                    return Err(ProtocolError::new(
                        ErrorCode::EffectDigestMismatch,
                        "this authorization is bound to a different effect than the one offered",
                    ))
                }
            }
        }

        if let Some(existing) = self
            .redemptions
            .iter()
            .find(|r| r.effect_key == request.effect_key)
        {
            // A retried agent turn. Same effect, same key, no second spend (C-13).
            return Ok((
                RedeemResult {
                    redeemed_at: existing.redeemed_at,
                    first_redemption: false,
                },
                None,
            ));
        }

        if self.single_use && !self.redemptions.is_empty() {
            return Err(ProtocolError::new(
                ErrorCode::AuthorizationSpent,
                "this authorization is single-use and has already been spent on another effect",
            ));
        }

        let redemption = Redemption {
            effect_key: request.effect_key.clone(),
            redeemed_at: now,
        };
        Ok((
            RedeemResult {
                redeemed_at: now,
                first_redemption: true,
            },
            Some(redemption),
        ))
    }

    /// Apply a redemption produced by [`Authorization::redeem`].
    ///
    /// A convenience for callers holding the authorization in memory; a durable Server writes the
    /// row instead, inside the same transaction as the check.
    pub fn apply(&mut self, redemption: Redemption) {
        self.redemptions.push(redemption);
    }
}

/// The digest an effect's parameters hash to, for [`AuthorizationBinding::effect_digest`].
///
/// A thin wrapper over [`crate::receipt::digest_of`], named for the job so a caller does not have to
/// remember which canonical form the binding compares.
pub fn effect_digest(parameters: &Value) -> Result<Digest> {
    crate::receipt::digest_of(parameters)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ts(s: &str) -> Timestamp {
        Timestamp::parse(s).expect("valid timestamp")
    }

    fn authorization() -> Authorization {
        Authorization::mint(
            AuthorizationId::parse("auth_01K3MB2R4ZC4YRXB2N6VD9FTHE").expect("parse"),
            ReceiptId::parse("rcpt_01K3MB2R4YC4YRXB2N6VD9FTHE").expect("parse"),
            RequestId::parse("req_01K3M7QW8ZC4YRXB2N6VD9FTHE").expect("parse"),
            json!({"decision": "approve"})
                .as_object()
                .expect("object")
                .clone(),
        )
    }

    fn redeem(key: &str) -> RedeemRequest {
        RedeemRequest {
            effect_key: key.to_string(),
            effect_digest: None,
        }
    }

    #[test]
    fn redemption_is_idempotent_per_effect_key() {
        // C-13, first half: redeem twice with the same key, `first_redemption` true then false.
        let mut auth = authorization();
        let now = ts("2026-07-30T14:07:44Z");

        let (first, record) = auth
            .redeem(&redeem("mailer:campaign:cmp_88213"), now)
            .expect("first spend");
        assert!(first.first_redemption);
        auth.apply(record.expect("a first redemption produces a record"));

        let later = ts("2026-07-30T14:09:00Z");
        let (replay, record) = auth
            .redeem(&redeem("mailer:campaign:cmp_88213"), later)
            .expect("replay");
        assert!(
            !replay.first_redemption,
            "a retried turn must not double-spend"
        );
        assert_eq!(
            replay.redeemed_at, now,
            "the replay reports the original spend, not now"
        );
        assert!(record.is_none(), "nothing further to persist");
    }

    #[test]
    fn a_single_use_authorization_cannot_be_spent_on_a_second_effect() {
        // C-13, second half.
        let mut auth = authorization();
        let now = ts("2026-07-30T14:07:44Z");
        let (_, record) = auth.redeem(&redeem("effect-a"), now).expect("first spend");
        auth.apply(record.expect("record"));

        let err = auth.redeem(&redeem("effect-b"), now).expect_err("refused");
        assert_eq!(err.code, ErrorCode::AuthorizationSpent);
        assert_eq!(err.http_status(), 409);
        assert_eq!(auth.state_at(now), AuthorizationState::Spent);
    }

    #[test]
    fn a_multi_use_authorization_accepts_distinct_effects() {
        let mut auth = authorization();
        auth.single_use = false;
        let now = ts("2026-07-30T14:07:44Z");
        for key in ["effect-a", "effect-b"] {
            let (result, record) = auth.redeem(&redeem(key), now).expect("spend");
            assert!(result.first_redemption);
            auth.apply(record.expect("record"));
        }
        assert_eq!(auth.redemptions.len(), 2);
        assert_eq!(auth.state_at(now), AuthorizationState::Open);
    }

    #[test]
    fn an_approval_of_one_amount_cannot_be_spent_on_another() {
        // The §10 example, made mechanical: approving "refund $2,400" must not authorize
        // "refund $24,000".
        let approved = json!({"action": "refund", "customer": "acme", "amount": "2400.00"});
        let inflated = json!({"action": "refund", "customer": "acme", "amount": "24000.00"});

        let auth = authorization().bound_to(AuthorizationBinding {
            waiter_ref: Some("run:0198f2a1".to_string()),
            effect_digest: Some(effect_digest(&approved).expect("digest")),
        });
        let now = ts("2026-07-30T14:07:44Z");

        let honest = RedeemRequest {
            effect_key: "refund:inv-8821".to_string(),
            effect_digest: Some(effect_digest(&approved).expect("digest")),
        };
        assert!(
            auth.redeem(&honest, now)
                .expect("matches")
                .0
                .first_redemption
        );

        let inflated_spend = RedeemRequest {
            effect_key: "refund:inv-8821".to_string(),
            effect_digest: Some(effect_digest(&inflated).expect("digest")),
        };
        let err = auth.redeem(&inflated_spend, now).expect_err("refused");
        assert_eq!(err.code, ErrorCode::EffectDigestMismatch);

        // Omitting the digest entirely must not slip past the binding either.
        assert_eq!(
            auth.redeem(&redeem("refund:inv-8821"), now)
                .expect_err("refused")
                .code,
            ErrorCode::EffectDigestMismatch
        );
    }

    #[test]
    fn the_binding_is_checked_before_the_replay_shortcut() {
        // Otherwise a caller could spend a mis-shaped effect by reusing a key that already
        // succeeded, and the digest check would never run.
        let approved = json!({"amount": "2400.00"});
        let mut auth = authorization().bound_to(AuthorizationBinding {
            waiter_ref: None,
            effect_digest: Some(effect_digest(&approved).expect("digest")),
        });
        let now = ts("2026-07-30T14:07:44Z");
        let good = RedeemRequest {
            effect_key: "refund:inv-8821".to_string(),
            effect_digest: Some(effect_digest(&approved).expect("digest")),
        };
        let (_, record) = auth.redeem(&good, now).expect("spend");
        auth.apply(record.expect("record"));

        let smuggled = RedeemRequest {
            effect_key: "refund:inv-8821".to_string(),
            effect_digest: Some(effect_digest(&json!({"amount": "24000.00"})).expect("digest")),
        };
        assert_eq!(
            auth.redeem(&smuggled, now).expect_err("refused").code,
            ErrorCode::EffectDigestMismatch
        );
    }

    #[test]
    fn an_unbound_authorization_ignores_an_offered_digest() {
        // Binding is opt-in (§10 rule 5). An unbound authorization must not start rejecting
        // callers that volunteer a digest.
        let auth = authorization();
        let now = ts("2026-07-30T14:07:44Z");
        let offered = RedeemRequest {
            effect_key: "effect-a".to_string(),
            effect_digest: Some(effect_digest(&json!({"anything": true})).expect("digest")),
        };
        assert!(
            auth.redeem(&offered, now)
                .expect("accepted")
                .0
                .first_redemption
        );
    }

    #[test]
    fn an_expired_authorization_is_no_longer_spendable() {
        let auth = authorization().expiring_at(ts("2026-07-31T14:07:44Z"));
        let before = ts("2026-07-31T14:07:43Z");
        let at = ts("2026-07-31T14:07:44Z");

        assert_eq!(auth.state_at(before), AuthorizationState::Open);
        assert!(auth.redeem(&redeem("effect-a"), before).is_ok());

        assert_eq!(auth.state_at(at), AuthorizationState::Expired);
        let err = auth.redeem(&redeem("effect-a"), at).expect_err("refused");
        assert!(err.message.contains("expiry"), "{}", err.message);
    }

    #[test]
    fn an_effect_key_must_be_present_and_bounded() {
        let auth = authorization();
        let now = ts("2026-07-30T14:07:44Z");
        for key in [String::new(), "x".repeat(257)] {
            let err = auth
                .redeem(
                    &RedeemRequest {
                        effect_key: key,
                        effect_digest: None,
                    },
                    now,
                )
                .expect_err("refused");
            assert_eq!(err.code, ErrorCode::InvalidRequest);
        }
    }

    #[test]
    fn an_authorization_names_exactly_one_receipt() {
        // I10 from the data side: the authorization is minted with its receipt and points at it.
        let auth = authorization();
        assert_eq!(
            auth.receipt_id.to_string(),
            "rcpt_01K3MB2R4YC4YRXB2N6VD9FTHE"
        );
        assert!(
            auth.single_use,
            "one answer authorizes exactly one effect by default"
        );
        assert_eq!(auth.grants["decision"], json!("approve"));
    }

    #[test]
    fn an_authorization_round_trips_through_the_wire_shape() {
        let mut auth = authorization().expiring_at(ts("2026-07-31T14:07:44Z"));
        auth.apply(Redemption {
            effect_key: "effect-a".to_string(),
            redeemed_at: ts("2026-07-30T14:07:44Z"),
        });
        let json = serde_json::to_value(&auth).expect("serialize");
        assert_eq!(json["single_use"], json!(true));
        assert_eq!(json["redemptions"][0]["effect_key"], json!("effect-a"));
        assert_eq!(
            serde_json::from_value::<Authorization>(json).expect("deserialize"),
            auth
        );
    }
}
