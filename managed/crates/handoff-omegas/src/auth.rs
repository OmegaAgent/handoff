//! `CallerAuthenticator` — and the first of the two contradictions the plan leaves open.
//!
//! # The contradiction
//!
//! PLAN:1212 lists machine auth as the **first** thing Handoff needs, and PLAN:718-745 then
//! specifies it as an `omg_<product>_<env>_<key_id>_<secret>` key verified by an `ApiKeyPrincipal`
//! **Axum extractor beside `AuthPrincipal` in `services/api/src/auth.rs`**, doing "one indexed
//! fetch" against an `api_keys` table.
//!
//! A service running on `handoff.omegas.dev` has neither. It is not in that Axum app, so it cannot
//! have that extractor; it has no connection to that database, so it cannot do that fetch. As
//! written, machine auth cannot serve an out-of-repo Handoff at all. **This is not a criticism of
//! the plan — it is what happens when a design written for an in-repo product meets the decision to
//! put the product out of repo — and it needs an owner decision, not a workaround.**
//!
//! # What this implements, and why
//!
//! The discovery weighed three ways out (13-open-closed-boundary §3.1.1) and recommended the second:
//!
//! | | Mechanism | Why not |
//! |---|---|---|
//! | (a) Introspection | Handoff calls `POST /api/keys/introspect` per request | Puts the control plane on Handoff's **hot path**. A product whose promise is a durable wait must not go down because the control plane is redeploying. |
//! | **(b) Token exchange** | The caller exchanges its `omg_` key for a short-lived ES256 JWT at `POST /api/token`, and Handoff verifies it **offline** against the existing JWKS | Recommended, and implemented here. |
//! | (c) Shared database | Handoff reads `api_keys` directly | Violates the plan's own rule: product code depends on control-plane **contracts**, never on control-plane tables. |
//!
//! (b) has the property the other two lack: **the open core gets the same code path.** The open
//! `CallerAuthenticator` default is "verify a JWT against a configured JWKS"; a self-hoster points
//! it at their own IdP and Ωmegas points it at `BETTER_AUTH_URL`. One implementation, two issuers,
//! and the managed path is not a special case. Options (a) and (c) would each give the hosted tier
//! an authentication mechanism the open core does not have, which is precisely the shape that makes
//! an open core rot.
//!
//! Nothing here changes the `omg_` key **format**, which is already decided and which secret
//! scanners recognise. It changes only what the key is presented *to*: **Handoff never sees the key
//! at all.** [`OmegasAuthenticator`] refuses one on sight, which is a security property and not
//! merely a nicety — a service that accepts long-lived keys will eventually be asked to store one.
//!
//! # The open decision, stated where it cannot be missed
//!
//! `POST /api/token` **does not exist**, and neither does a JWKS this service is entitled to use.
//! Machine auth is M5, after M0–M4. Until an owner rules on the exchange, this adapter authenticates
//! nobody and says why: with no issuer configured every credential is refused with
//! [`MissingDependency::TOKEN_EXCHANGE`]. **Handoff's public API is blocked on that decision.**
//!
//! # Verification order is the security property
//!
//! The steps below run in this order and no other, because several classic JWT breaks are ordering
//! bugs rather than crypto bugs:
//!
//! 1. Refuse anything shaped like a raw `omg_` key.
//! 2. Read the header and **pin `alg` to `ES256` before any signature work** — this is what stops
//!    `alg: none` and RS256/HS256 confusion, and it must happen before a key is even looked up.
//! 3. Require `kid`; resolve it, refetching the JWKS at most once on a miss.
//! 4. Verify the signature over `header.payload`.
//! 5. **Only then** read the claims. Expiry, issuer, and audience are checked after the signature,
//!    because an unverified claim is an attacker's input.

use handoff_core::auth::{Principal, PrincipalKind};
use handoff_core::ports::BoxFuture;
use handoff_core::seam::CallerAuthenticator;
use handoff_protocol::error::{ErrorCode, ProtocolError, Result};
use handoff_protocol::id::PrincipalId;
use handoff_protocol::requires::{AuthStrength, Role};
use serde::Deserialize;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::dependency::MissingDependency;

/// The one algorithm this verifier accepts.
///
/// Pinned as a constant rather than read from configuration: a deployment-configurable algorithm is
/// a deployment-configurable downgrade.
pub const ALGORITHM: &str = "ES256";

/// How much clock disagreement between two services is tolerated on `exp` and `nbf`.
pub const CLOCK_SKEW: Duration = Duration::from_secs(60);

/// One key from a JWKS document.
#[derive(Debug, Clone, Deserialize)]
pub struct Jwk {
    /// Key id, matched against the token header's `kid`.
    pub kid: String,
    /// Key type. Must be `EC`.
    pub kty: String,
    /// Curve. Must be `P-256`.
    #[serde(default)]
    pub crv: String,
    /// The x coordinate, base64url.
    #[serde(default)]
    pub x: String,
    /// The y coordinate, base64url.
    #[serde(default)]
    pub y: String,
}

/// A JWKS document.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Jwks {
    /// The keys.
    pub keys: Vec<Jwk>,
}

/// Where the public keys come from.
pub trait JwksSource: Send + Sync {
    /// Fetch the current document.
    fn fetch(&self) -> BoxFuture<'_, Result<Jwks>>;
}

/// The real source: an HTTPS GET against the issuer's JWKS URL.
pub struct HttpJwks {
    url: String,
    client: reqwest::Client,
}

impl HttpJwks {
    /// Point at a JWKS URL.
    pub fn new(url: impl Into<String>) -> Result<Self> {
        Ok(Self {
            url: url.into(),
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(5))
                .build()
                .map_err(|e| {
                    ProtocolError::new(
                        ErrorCode::InvalidRequest,
                        format!("cannot build the JWKS client: {e}"),
                    )
                })?,
        })
    }
}

impl JwksSource for HttpJwks {
    fn fetch(&self) -> BoxFuture<'_, Result<Jwks>> {
        Box::pin(async move {
            let response = self.client.get(&self.url).send().await.map_err(|e| {
                ProtocolError::new(
                    ErrorCode::DeliveryUnavailable,
                    format!("cannot reach the JWKS at {}: {e}", self.url),
                )
            })?;
            response.json::<Jwks>().await.map_err(|e| {
                ProtocolError::new(
                    ErrorCode::DeliveryUnavailable,
                    format!("the JWKS at {} did not parse: {e}", self.url),
                )
            })
        })
    }
}

/// A fixed set of keys, for tests.
pub struct StaticJwks {
    keys: Jwks,
    fetches: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

impl StaticJwks {
    /// Hold these keys.
    pub fn new(keys: Jwks) -> Self {
        Self {
            keys,
            fetches: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        }
    }

    /// A handle to the fetch count, which outlives the boxing of this source into the verifier.
    pub fn counter(&self) -> std::sync::Arc<std::sync::atomic::AtomicUsize> {
        std::sync::Arc::clone(&self.fetches)
    }
}

impl JwksSource for StaticJwks {
    fn fetch(&self) -> BoxFuture<'_, Result<Jwks>> {
        self.fetches
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let keys = self.keys.clone();
        Box::pin(async move { Ok(keys) })
    }
}

/// What this service will accept in a token.
#[derive(Debug, Clone)]
pub struct TokenPolicy {
    /// The issuer every token must name. Empty means "no issuer is configured", which means this
    /// deployment authenticates nobody.
    pub issuer: String,
    /// The audience every token must name — this service.
    pub audience: String,
}

/// The `CallerAuthenticator` for the hosted deployment.
pub struct OmegasAuthenticator {
    source: Box<dyn JwksSource>,
    policy: TokenPolicy,
    cached: Mutex<Option<Jwks>>,
}

impl OmegasAuthenticator {
    /// Build one against a JWKS source.
    pub fn new(source: Box<dyn JwksSource>, policy: TokenPolicy) -> Self {
        Self {
            source,
            policy,
            cached: Mutex::new(None),
        }
    }

    /// The key for a `kid`, refetching **once** on a miss.
    ///
    /// One refetch, not unbounded: a token carrying an unknown `kid` is the cheapest way to make a
    /// verifier hammer its issuer, so the miss path has to terminate.
    async fn key_for(&self, kid: &str) -> Result<Jwk> {
        if let Some(found) = self.cached_key(kid) {
            return Ok(found);
        }
        let fetched = self.source.fetch().await?;
        *self.cached.lock().expect("jwks lock") = Some(fetched);
        self.cached_key(kid).ok_or_else(|| {
            ProtocolError::new(
                ErrorCode::InvalidApiKey,
                "the token names a signing key this service does not know",
            )
        })
    }

    fn cached_key(&self, kid: &str) -> Option<Jwk> {
        self.cached
            .lock()
            .expect("jwks lock")
            .as_ref()?
            .keys
            .iter()
            .find(|k| k.kid == kid)
            .cloned()
    }

    async fn verify(&self, token: &str, now: u64) -> Result<Principal> {
        if self.policy.issuer.is_empty() {
            return Err(MissingDependency::TOKEN_EXCHANGE.into_error());
        }

        // 1. A long-lived key must never reach this service. Refusing it explicitly, with the
        //    remedy in the message, is better than a generic rejection that reads like a bug.
        if token.starts_with("omg_") {
            return Err(ProtocolError::new(
                ErrorCode::InvalidApiKey,
                "this service does not accept omg_ API keys. Exchange the key for a short-lived \
                 token at POST /api/token on the control plane and present that instead; Handoff \
                 verifies it offline and never holds your key.",
            ));
        }

        let parts: Vec<&str> = token.split('.').collect();
        if parts.len() != 3 {
            return Err(invalid("the credential is not a JWS compact serialization"));
        }

        // 2. Pin the algorithm BEFORE anything else touches a key. `alg: none` and RS256/HS256
        //    confusion are both defeated here, and only here.
        let header: TokenHeader = decode_json(parts[0], "header")?;
        if header.alg != ALGORITHM {
            return Err(invalid(format!(
                "this service verifies {ALGORITHM} only; the token declares `{}`",
                header.alg
            )));
        }
        let Some(kid) = header.kid else {
            return Err(invalid("the token header carries no `kid`"));
        };

        // 3 and 4. Resolve the key and verify, over the exact bytes that were signed.
        let jwk = self.key_for(&kid).await?;
        verify_es256(&jwk, parts[0], parts[1], parts[2])?;

        // 5. The claims are trustworthy only now.
        let claims: TokenClaims = decode_json(parts[1], "claims")?;
        if claims.iss != self.policy.issuer {
            return Err(invalid("the token was issued by someone else"));
        }
        if !claims.audience().iter().any(|a| a == &self.policy.audience) {
            return Err(invalid("the token was not issued for this service"));
        }
        let skew = CLOCK_SKEW.as_secs();
        if claims.exp.saturating_add(skew) <= now {
            return Err(invalid("the token has expired"));
        }
        if let Some(nbf) = claims.nbf {
            if nbf > now.saturating_add(skew) {
                return Err(invalid("the token is not valid yet"));
            }
        }

        claims.into_principal()
    }
}

impl CallerAuthenticator for OmegasAuthenticator {
    fn authenticate(&self, presented_secret: String) -> BoxFuture<'_, Result<Option<Principal>>> {
        Box::pin(async move {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            match self.verify(&presented_secret, now).await {
                Ok(principal) => Ok(Some(principal)),
                // A credential that is genuinely not valid is `None`. Anything else — an unreachable
                // JWKS, an unconfigured issuer — is an error, because telling a caller their key is
                // invalid during our own outage sends them to rotate a key that was never wrong.
                Err(error) if error.code == ErrorCode::InvalidApiKey => Ok(None),
                Err(error) => Err(error),
            }
        })
    }
}

#[derive(Debug, Deserialize)]
struct TokenHeader {
    alg: String,
    #[serde(default)]
    kid: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TokenClaims {
    iss: String,
    sub: String,
    #[serde(default)]
    aud: serde_json::Value,
    exp: u64,
    #[serde(default)]
    nbf: Option<u64>,
    /// The tenant. This is the only place a tenant may come from (§4.1, I13).
    #[serde(default, alias = "org_id")]
    org: Option<String>,
    /// OAuth2-style space-delimited scopes.
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    role: Option<String>,
}

impl TokenClaims {
    fn audience(&self) -> Vec<String> {
        match &self.aud {
            serde_json::Value::String(one) => vec![one.clone()],
            serde_json::Value::Array(many) => many
                .iter()
                .filter_map(|a| a.as_str().map(str::to_string))
                .collect(),
            _ => Vec::new(),
        }
    }

    fn into_principal(self) -> Result<Principal> {
        let Some(org) = self.org else {
            return Err(invalid(
                "the token carries no organization claim, and this service will not infer one",
            ));
        };
        // Refuse a tenant that cannot be written onto a receipt. Finding that out here is better
        // than finding it out when the first person answers.
        handoff_core::plan::tenant_as_org(&org)?;

        let id = PrincipalId::parse(&self.sub)
            .map_err(|_| invalid("the token's subject is not a principal identifier"))?;

        // The kind comes from the subject's own **type**, never from a claim. §4.2 requires that no
        // role, scope, setting, or deployment mode can make a machine an answerer; reading a `kind`
        // claim would make it one signed token away from configurable.
        let kind = if id.is_machine() {
            PrincipalKind::Machine
        } else {
            PrincipalKind::Human
        };

        let role = match self.role.as_deref() {
            Some("admin") | Some("owner") => Role::Admin,
            Some("editor") | Some("member") => Role::Editor,
            _ => Role::Viewer,
        };

        let scopes = self
            .scope
            .unwrap_or_default()
            .split_whitespace()
            .map(str::to_string)
            .collect::<Vec<_>>();

        Ok(Principal {
            id: Some(id),
            kind,
            tenant_ref: org,
            role,
            // A token exchange establishes a session and nothing stronger. A hosted deployment must
            // not mint `reauth` or `mfa` out of a bearer token it did not watch being obtained.
            auth_strength: AuthStrength::Session,
            display: None,
            scopes,
        })
    }
}

fn invalid(message: impl Into<String>) -> ProtocolError {
    ProtocolError::new(ErrorCode::InvalidApiKey, message)
}

fn b64(segment: &str) -> Result<Vec<u8>> {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(segment)
        .map_err(|_| invalid("a token segment is not valid base64url"))
}

fn decode_json<T: serde::de::DeserializeOwned>(segment: &str, what: &str) -> Result<T> {
    serde_json::from_slice(&b64(segment)?)
        .map_err(|_| invalid(format!("the token {what} is not JSON")))
}

fn verify_es256(jwk: &Jwk, header: &str, payload: &str, signature: &str) -> Result<()> {
    use p256::ecdsa::signature::Verifier;

    if jwk.kty != "EC" || jwk.crv != "P-256" {
        return Err(invalid("the signing key is not a P-256 EC key"));
    }
    let x = b64(&jwk.x)?;
    let y = b64(&jwk.y)?;
    if x.len() != 32 || y.len() != 32 {
        return Err(invalid(
            "the signing key's coordinates are the wrong length",
        ));
    }
    let mut sec1 = Vec::with_capacity(65);
    sec1.push(0x04);
    sec1.extend_from_slice(&x);
    sec1.extend_from_slice(&y);

    let key = p256::ecdsa::VerifyingKey::from_sec1_bytes(&sec1)
        .map_err(|_| invalid("the signing key is not a point on P-256"))?;

    let raw = b64(signature)?;
    // JWS ES256 is the fixed 64-byte r‖s form, never DER. Accepting DER here would accept two
    // encodings of one signature, which is a malleability surface for no benefit.
    let parsed = p256::ecdsa::Signature::from_slice(&raw)
        .map_err(|_| invalid("the signature is not a 64-byte ES256 signature"))?;

    let signed = format!("{header}.{payload}");
    key.verify(signed.as_bytes(), &parsed)
        .map_err(|_| invalid("the token signature does not verify"))
}

#[cfg(test)]
pub(crate) mod testing {
    //! A local issuer, so the verifier is tested end to end without the control plane existing.

    use super::*;
    use base64::Engine;
    use p256::ecdsa::{signature::Signer, SigningKey};

    pub const KID: &str = "test-key-1";
    pub const ISSUER: &str = "https://auth.omegas.dev";
    pub const AUDIENCE: &str = "https://handoff.omegas.dev";

    fn encode(bytes: &[u8]) -> String {
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
    }

    pub fn signing_key() -> SigningKey {
        // A fixed scalar, so the suite is deterministic and needs no RNG.
        SigningKey::from_slice(&[7u8; 32]).expect("a valid P-256 scalar")
    }

    pub fn jwks() -> Jwks {
        let verifying = signing_key().verifying_key().to_encoded_point(false);
        Jwks {
            keys: vec![Jwk {
                kid: KID.into(),
                kty: "EC".into(),
                crv: "P-256".into(),
                x: encode(verifying.x().expect("x")),
                y: encode(verifying.y().expect("y")),
            }],
        }
    }

    /// Sign a token with an arbitrary header and claims, so a test can build a bad one on purpose.
    pub fn sign(header: serde_json::Value, claims: serde_json::Value) -> String {
        let header = encode(header.to_string().as_bytes());
        let payload = encode(claims.to_string().as_bytes());
        let signature: p256::ecdsa::Signature =
            signing_key().sign(format!("{header}.{payload}").as_bytes());
        format!("{header}.{payload}.{}", encode(&signature.to_bytes()))
    }

    pub fn claims() -> serde_json::Value {
        serde_json::json!({
            "iss": ISSUER,
            "aud": AUDIENCE,
            "sub": "sa_01K3M7QW8ZC4YRXB2N6VD9FTHE",
            "org": "org_01K3M7QW8ZC4YRXB2N6VD9FTHE",
            "scope": "handoff:requests:write handoff:requests:read",
            "exp": 2_000_000_000u64,
        })
    }

    pub fn header() -> serde_json::Value {
        serde_json::json!({ "alg": "ES256", "kid": KID, "typ": "JWT" })
    }

    pub fn authenticator() -> OmegasAuthenticator {
        OmegasAuthenticator::new(
            Box::new(StaticJwks::new(jwks())),
            TokenPolicy {
                issuer: ISSUER.into(),
                audience: AUDIENCE.into(),
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::testing::*;
    use super::*;

    const NOW: u64 = 1_900_000_000;

    #[tokio::test]
    async fn a_well_formed_token_authenticates_a_machine_principal() {
        let token = sign(header(), claims());
        let principal = authenticator().verify(&token, NOW).await.expect("verify");
        assert_eq!(principal.kind, PrincipalKind::Machine);
        assert_eq!(principal.tenant_ref, "org_01K3M7QW8ZC4YRXB2N6VD9FTHE");
        assert!(principal.has_scope("handoff:requests:write"));
        assert!(!principal.has_scope("handoff:requests:admin"));
        // §4.2 holds no matter what the issuer said.
        assert!(!principal.may_answer());
    }

    #[tokio::test]
    async fn a_raw_omg_key_is_refused_with_the_remedy_rather_than_a_bare_rejection() {
        let error = authenticator()
            .verify("omg_handoff_prod_ABC_DEF", NOW)
            .await
            .expect_err("a long-lived key must never reach this service");
        assert!(error.message.contains("POST /api/token"));
        assert!(error.message.contains("never holds your key"));
    }

    #[tokio::test]
    async fn alg_none_is_refused_before_any_key_is_looked_up() {
        let source = StaticJwks::new(jwks());
        let authenticator = OmegasAuthenticator::new(
            Box::new(source),
            TokenPolicy {
                issuer: ISSUER.into(),
                audience: AUDIENCE.into(),
            },
        );
        let unsigned = {
            use base64::Engine;
            let encode = |v: serde_json::Value| {
                base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(v.to_string().as_bytes())
            };
            format!(
                "{}.{}.",
                encode(serde_json::json!({"alg":"none","kid":KID})),
                encode(claims())
            )
        };
        let error = authenticator
            .verify(&unsigned, NOW)
            .await
            .expect_err("alg none must never verify");
        assert!(error.message.contains("verifies ES256 only"));
    }

    #[tokio::test]
    async fn an_hs256_token_is_refused_rather_than_verified_against_the_public_key() {
        // The classic confusion: treat the EC public key as an HMAC secret. Pinning the algorithm
        // before a key is resolved is what stops it, so this asserts the message from that step.
        let error = authenticator()
            .verify(
                &sign(serde_json::json!({"alg":"HS256","kid":KID}), claims()),
                NOW,
            )
            .await
            .expect_err("an algorithm swap must be refused");
        assert!(error.message.contains("verifies ES256 only"));
    }

    #[tokio::test]
    async fn a_token_with_no_kid_is_refused() {
        let error = authenticator()
            .verify(&sign(serde_json::json!({"alg":"ES256"}), claims()), NOW)
            .await
            .expect_err("a kid is required");
        assert!(error.message.contains("no `kid`"));
    }

    #[tokio::test]
    async fn a_tampered_payload_does_not_verify() {
        let token = sign(header(), claims());
        let mut parts: Vec<String> = token.split('.').map(str::to_string).collect();
        let mut forged = claims();
        forged["org"] = serde_json::json!("org_01K3M7QW8ZC4YRXB2N6VD9FTHF");
        parts[1] = {
            use base64::Engine;
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(forged.to_string().as_bytes())
        };
        let error = authenticator()
            .verify(&parts.join("."), NOW)
            .await
            .expect_err("swapping the tenant must not verify");
        assert!(error.message.contains("does not verify"));
    }

    #[tokio::test]
    async fn expiry_issuer_and_audience_are_all_checked() {
        let mut expired = claims();
        expired["exp"] = serde_json::json!(NOW - 3_600);
        assert!(authenticator()
            .verify(&sign(header(), expired), NOW)
            .await
            .expect_err("expired")
            .message
            .contains("expired"));

        let mut other_issuer = claims();
        other_issuer["iss"] = serde_json::json!("https://evil.example");
        assert!(authenticator()
            .verify(&sign(header(), other_issuer), NOW)
            .await
            .expect_err("wrong issuer")
            .message
            .contains("issued by someone else"));

        let mut other_audience = claims();
        other_audience["aud"] = serde_json::json!("https://someone-else.example");
        assert!(authenticator()
            .verify(&sign(header(), other_audience), NOW)
            .await
            .expect_err("wrong audience")
            .message
            .contains("not issued for this service"));
    }

    #[tokio::test]
    async fn a_token_with_no_organization_claim_is_refused_rather_than_defaulted() {
        // Inferring a tenant is how one tenant reads another's requests. There is no default.
        let mut no_org = claims();
        no_org.as_object_mut().expect("object").remove("org");
        let error = authenticator()
            .verify(&sign(header(), no_org), NOW)
            .await
            .expect_err("no org claim");
        assert!(error.message.contains("will not infer one"));
    }

    #[tokio::test]
    async fn a_tenant_that_cannot_be_written_on_a_receipt_is_refused_at_the_door() {
        let mut bad_org = claims();
        bad_org["org"] = serde_json::json!("acme-corp");
        assert!(authenticator()
            .verify(&sign(header(), bad_org), NOW)
            .await
            .is_err());
    }

    #[tokio::test]
    async fn an_unknown_kid_refetches_once_and_then_stops() {
        // An unknown `kid` is the cheapest way to make a verifier hammer its issuer, so the miss
        // path has to terminate. One refetch per miss, never a loop.
        let source = StaticJwks::new(jwks());
        let fetches = source.counter();
        let authenticator = OmegasAuthenticator::new(
            Box::new(source),
            TokenPolicy {
                issuer: ISSUER.into(),
                audience: AUDIENCE.into(),
            },
        );

        let unknown = sign(serde_json::json!({"alg":"ES256","kid":"who?"}), claims());
        for _ in 0..3 {
            assert!(authenticator.verify(&unknown, NOW).await.is_err());
        }
        assert_eq!(
            fetches.load(std::sync::atomic::Ordering::SeqCst),
            3,
            "one fetch per miss, and no retry loop inside a single call"
        );
    }

    #[tokio::test]
    async fn a_known_kid_is_served_from_cache_after_the_first_fetch() {
        let source = StaticJwks::new(jwks());
        let fetches = source.counter();
        let authenticator = OmegasAuthenticator::new(
            Box::new(source),
            TokenPolicy {
                issuer: ISSUER.into(),
                audience: AUDIENCE.into(),
            },
        );
        let token = sign(header(), claims());
        for _ in 0..5 {
            authenticator.verify(&token, NOW).await.expect("verify");
        }
        assert_eq!(
            fetches.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "the hot path must be offline, or the control plane is back on it"
        );
    }

    #[tokio::test]
    async fn with_no_issuer_configured_this_adapter_authenticates_nobody_and_says_why() {
        // This is the state the service is actually in today: M5 has not landed, so there is no
        // exchange endpoint and no issuer. Refusing loudly is the honest behaviour.
        let authenticator = OmegasAuthenticator::new(
            Box::new(StaticJwks::new(Jwks::default())),
            TokenPolicy {
                issuer: String::new(),
                audience: AUDIENCE.into(),
            },
        );
        let error = authenticator
            .verify(&sign(header(), claims()), NOW)
            .await
            .expect_err("no issuer configured");
        assert!(error.message.contains("POST /api/token"));
        assert!(error.message.contains("M5"));
    }

    #[tokio::test]
    async fn an_invalid_credential_is_none_but_an_outage_is_an_error() {
        // The distinction the port documents: `None` means "not valid", an error means "we could
        // not find out". Collapsing them tells a caller to rotate a key that was never wrong.
        let authenticator = authenticator();
        assert!(authenticator
            .authenticate("not-a-token".into())
            .await
            .expect("a malformed credential is simply invalid")
            .is_none());

        let unconfigured = OmegasAuthenticator::new(
            Box::new(StaticJwks::new(Jwks::default())),
            TokenPolicy {
                issuer: String::new(),
                audience: AUDIENCE.into(),
            },
        );
        assert!(unconfigured.authenticate("anything".into()).await.is_err());
    }
}
