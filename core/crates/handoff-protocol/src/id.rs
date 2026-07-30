//! Identifiers: `<prefix>_<26-character Crockford base32 ULID>` (§1.4).
//!
//! Three properties matter, and each is enforced here rather than assumed:
//!
//! 1. **Typed.** `Id<Request>` and `Id<Receipt>` are different types, so a delivery id cannot be
//!    passed where a receipt id belongs. The prefix set is closed and sealed.
//! 2. **Time-sortable and safe to log.** Crockford base32 omits `I`, `L`, `O`, and `U`, so an id
//!    survives being read aloud. The leading 48 bits are milliseconds since the Unix epoch.
//! 3. **Never an authorization.** Possession of an identifier grants nothing (§4.6, I17). Nothing
//!    in this module makes an id secret, because nothing anywhere may depend on it being secret.
//!
//! Generation is pure: the caller supplies the time and the entropy. A crate with no I/O cannot
//! read a clock or a random device, which is exactly why those are parameters.

use crate::error::{ErrorCode, ProtocolError, Result};
use serde::{de, Deserialize, Deserializer, Serialize, Serializer};
use std::cmp::Ordering;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::marker::PhantomData;

/// Crockford base32, in value order. `I`, `L`, `O`, and `U` are absent by construction.
const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// The encoded length of a 128-bit ULID in base32.
const ENCODED_LEN: usize = 26;

/// Decode one Crockford character, strictly: canonical uppercase only.
///
/// Crockford's own spec allows case-insensitive input and treats `I`/`L` as `1` and `O` as `0`. We
/// do not, because an id that has two spellings has two digests, and ids go inside receipts that
/// get hashed. Fail closed and let the caller send the canonical form.
const fn decode_char(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'A'..=b'H' => Some(c - b'A' + 10),
        b'J' | b'K' => Some(c - b'J' + 18),
        b'M' | b'N' => Some(c - b'M' + 20),
        b'P'..=b'T' => Some(c - b'P' + 22),
        b'V'..=b'Z' => Some(c - b'V' + 27),
        _ => None,
    }
}

mod sealed {
    /// Closes [`super::IdKind`]: the prefix vocabulary is defined by the specification, and a
    /// downstream crate inventing an eleventh prefix would be inventing protocol.
    pub trait Sealed {}
}

/// One kind of identifier, carrying its wire prefix.
///
/// Sealed: §1.4 enumerates the prefixes and they are all REQUIRED, so the set is not open.
pub trait IdKind: sealed::Sealed {
    /// The lowercase prefix that precedes the underscore.
    const PREFIX: &'static str;
}

macro_rules! id_kinds {
    ($(
        $(#[$kind_doc:meta])*
        $kind:ident => $prefix:literal, $(#[$alias_doc:meta])* $alias:ident
    );+ $(;)?) => {
        $(
            $(#[$kind_doc])*
            ///
            /// A type-level marker only; it has no values.
            #[derive(Debug)]
            pub enum $kind {}
            impl sealed::Sealed for $kind {}
            impl IdKind for $kind {
                const PREFIX: &'static str = $prefix;
            }
            $(#[$alias_doc])*
            pub type $alias = Id<$kind>;
        )+

        /// Every identifier prefix the specification defines, in §1.4 order.
        ///
        /// Exposed so a Server can validate an id of unknown kind without a `match` over prefixes.
        pub const ALL_PREFIXES: &[&str] = &[$($prefix),+];
    };
}

id_kinds! {
    /// A human-intervention request.
    Request => "req", /// Identifies a [`Request`].
    RequestId;
    /// An immutable receipt.
    Receipt => "rcpt", /// Identifies a [`Receipt`].
    ReceiptId;
    /// A spendable authorization.
    Authorization => "auth", /// Identifies an [`Authorization`].
    AuthorizationId;
    /// One attempt-bearing send to one target on one channel.
    Delivery => "dlv", /// Identifies a [`Delivery`].
    DeliveryId;
    /// One queued notification to a waiter.
    Signal => "sig", /// Identifies a [`Signal`].
    SignalId;
    /// A capability grant handle. Opaque, and never derived from anything recomputable (§11.1).
    Grant => "hg", /// Identifies a [`Grant`].
    GrantHandle;
    /// A short-lived, lease-bound session produced by resolving a grant.
    GrantSession => "hs", /// Identifies a [`GrantSession`].
    GrantSessionRef;
    /// A runtime-owned destination for `secret`-typed values.
    Sink => "snk", /// Identifies a [`Sink`].
    SinkRef;
    /// Proof that the acking client is the waiter a signal was enqueued for.
    Resume => "rt", /// Identifies a [`Resume`] token.
    ResumeToken;
    /// A person.
    User => "usr", /// Identifies a [`User`].
    UserId;
    /// A machine principal. It has no user identity, which is what makes
    /// `requester_may_not_answer` decidable by type (§4.2).
    ServiceAccount => "sa", /// Identifies a [`ServiceAccount`].
    ServiceAccountId;
    /// The tenant. Always derived from the credential, never from a request body (§4.1, I13).
    Org => "org", /// Identifies an [`Org`].
    OrgId;
}

/// A typed protocol identifier.
///
/// Stored as the 128-bit ULID and rendered on demand, so only the canonical spelling can ever
/// exist: there is exactly one string form per value, which is what a hash chain needs.
pub struct Id<K: IdKind> {
    value: u128,
    kind: PhantomData<fn() -> K>,
}

impl<K: IdKind> Id<K> {
    /// The maximum representable ULID timestamp: 2^48 - 1 milliseconds since the epoch.
    pub const MAX_TIMESTAMP_MS: u64 = (1 << 48) - 1;

    /// Build a time-sortable identifier from an explicit time and 80 bits of entropy.
    ///
    /// Both are parameters because this crate reads neither a clock nor a random device. The
    /// caller — `handoff-core`, in the reference implementation — owns both.
    pub fn from_parts(timestamp_ms: u64, entropy: [u8; 10]) -> Result<Self> {
        if timestamp_ms > Self::MAX_TIMESTAMP_MS {
            return Err(ProtocolError::new(
                ErrorCode::InvalidRequest,
                format!("timestamp {timestamp_ms} does not fit in a 48-bit ULID time component"),
            ));
        }
        let mut value = u128::from(timestamp_ms) << 80;
        for (i, byte) in entropy.iter().enumerate() {
            value |= u128::from(*byte) << (72 - i * 8);
        }
        Ok(Self {
            value,
            kind: PhantomData,
        })
    }

    /// Build an identifier from 128 bits of cryptographically secure randomness.
    ///
    /// This is the constructor for a [`GrantHandle`]: §11.1 requires a grant handle to come from a
    /// CSPRNG and forbids deriving it from a resource name or a shared secret, because a derived
    /// handle cannot be revoked individually. The top two bits are cleared so the value fits the
    /// 26-character encoding; the remaining 126 bits are unguessable.
    ///
    /// The result is **not** time-sortable, and `timestamp_ms` on it is meaningless.
    pub fn from_random(bytes: [u8; 16]) -> Self {
        let value = u128::from_be_bytes(bytes) & (u128::MAX >> 2);
        Self {
            value,
            kind: PhantomData,
        }
    }

    /// The raw 128-bit value.
    pub const fn to_u128(self) -> u128 {
        self.value
    }

    /// Milliseconds since the Unix epoch, from the leading 48 bits.
    ///
    /// Meaningful only for an id built by [`Id::from_parts`].
    pub const fn timestamp_ms(self) -> u64 {
        (self.value >> 80) as u64
    }

    /// The 26-character body, without the prefix or the underscore.
    pub fn encoded(self) -> String {
        let mut buf = [0u8; ENCODED_LEN];
        let mut value = self.value;
        for slot in buf.iter_mut().rev() {
            *slot = ALPHABET[(value & 0x1f) as usize];
            value >>= 5;
        }
        debug_assert_eq!(value, 0, "26 base32 characters cover all 128 bits");
        String::from_utf8(buf.to_vec()).expect("the Crockford alphabet is ASCII")
    }

    /// Parse `<prefix>_<26 chars>`, requiring this exact kind's prefix.
    ///
    /// Fails closed on a wrong prefix, a wrong length, a lowercase or ambiguous character, or a
    /// leading character above `7` (which would need a 131st bit).
    pub fn parse(s: &str) -> Result<Self> {
        fn bad(s: &str, why: &str) -> ProtocolError {
            ProtocolError::new(
                ErrorCode::InvalidRequest,
                format!("`{s}` is not a valid identifier: {why}"),
            )
        }

        let expected = K::PREFIX;
        let body = s
            .strip_prefix(expected)
            .and_then(|rest| rest.strip_prefix('_'))
            .ok_or_else(|| bad(s, &format!("expected the `{expected}_` prefix")))?;

        let bytes = body.as_bytes();
        if bytes.len() != ENCODED_LEN {
            return Err(bad(s, "the body must be exactly 26 characters"));
        }
        let mut value: u128 = 0;
        for (i, &c) in bytes.iter().enumerate() {
            let digit = decode_char(c)
                .ok_or_else(|| bad(s, "the body must be canonical uppercase Crockford base32"))?;
            if i == 0 && digit > 7 {
                return Err(bad(s, "the leading character must not exceed `7`"));
            }
            value = (value << 5) | u128::from(digit);
        }
        Ok(Self {
            value,
            kind: PhantomData,
        })
    }
}

impl<K: IdKind> fmt::Display for Id<K> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}_{}", K::PREFIX, self.encoded())
    }
}

impl<K: IdKind> fmt::Debug for Id<K> {
    /// Debug and Display agree. An identifier is ordinary data and safe to log (§1.4), so there is
    /// nothing here to redact.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self}")
    }
}

// The manual impls below exist so that `Id<K>` is `Copy`, `Ord`, and `Hash` regardless of what `K`
// is — the marker types are uninhabited and derive would demand bounds on them for no reason.
impl<K: IdKind> Clone for Id<K> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<K: IdKind> Copy for Id<K> {}
impl<K: IdKind> PartialEq for Id<K> {
    fn eq(&self, other: &Self) -> bool {
        self.value == other.value
    }
}
impl<K: IdKind> Eq for Id<K> {}
impl<K: IdKind> PartialOrd for Id<K> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl<K: IdKind> Ord for Id<K> {
    /// Numeric order, which for a time-sortable id is also chronological order.
    fn cmp(&self, other: &Self) -> Ordering {
        self.value.cmp(&other.value)
    }
}
impl<K: IdKind> Hash for Id<K> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.value.hash(state);
    }
}

impl<K: IdKind> Serialize for Id<K> {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de, K: IdKind> Deserialize<'de> for Id<K> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Id::<K>::parse(&raw).map_err(de::Error::custom)
    }
}

/// An authenticated subject: a person, a service account, or a tenant acting as one.
///
/// The variants are distinct types rather than a string with a prefix because the requester ≠
/// decider rule is enforced *by principal type* (§4.2, I15). If "is this a machine?" were a runtime
/// string comparison it would be one refactor away from being configurable, and §4.2 says there
/// MUST NOT be any configuration under which a machine can answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PrincipalId {
    /// A person.
    User(UserId),
    /// A machine principal. Can never answer a request.
    ServiceAccount(ServiceAccountId),
    /// A tenant acting as a principal.
    Org(OrgId),
}

impl PrincipalId {
    /// Whether this principal is a machine, and therefore may never answer (§4.2).
    pub const fn is_machine(self) -> bool {
        matches!(self, Self::ServiceAccount(_) | Self::Org(_))
    }

    /// Whether this principal is a person, and therefore may answer if authority permits.
    pub const fn is_person(self) -> bool {
        matches!(self, Self::User(_))
    }

    /// Parse any of the three principal prefixes.
    pub fn parse(s: &str) -> Result<Self> {
        UserId::parse(s)
            .map(Self::User)
            .or_else(|_| ServiceAccountId::parse(s).map(Self::ServiceAccount))
            .or_else(|_| OrgId::parse(s).map(Self::Org))
            .map_err(|_| {
                ProtocolError::new(
                    ErrorCode::InvalidRequest,
                    format!("`{s}` is not a principal identifier (`usr_`, `sa_`, or `org_`)"),
                )
            })
    }
}

impl fmt::Display for PrincipalId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::User(id) => write!(f, "{id}"),
            Self::ServiceAccount(id) => write!(f, "{id}"),
            Self::Org(id) => write!(f, "{id}"),
        }
    }
}

impl Serialize for PrincipalId {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for PrincipalId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        PrincipalId::parse(&raw).map_err(de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ENTROPY: [u8; 10] = [0x9f, 0x2c, 0x4b, 0x1e, 0x77, 0xa0, 0xe5, 0x6f, 0xf5, 0x36];

    #[test]
    fn ids_render_with_their_prefix_and_twenty_six_characters() {
        let id = RequestId::from_parts(1_785_000_131_000, ENTROPY).expect("in range");
        let rendered = id.to_string();
        assert!(rendered.starts_with("req_"), "{rendered}");
        assert_eq!(rendered.len(), "req_".len() + 26);
        assert_eq!(RequestId::parse(&rendered).expect("round trip"), id);
    }

    #[test]
    fn ids_are_time_sortable() {
        let earlier = RequestId::from_parts(1_785_000_131_000, [0xff; 10]).expect("in range");
        let later = RequestId::from_parts(1_785_000_132_000, [0x00; 10]).expect("in range");
        assert!(
            earlier < later,
            "a later id must sort after an earlier one whatever the entropy"
        );
        assert!(
            earlier.to_string() < later.to_string(),
            "lexicographic order must agree"
        );
        assert_eq!(earlier.timestamp_ms(), 1_785_000_131_000);
    }

    #[test]
    fn the_alphabet_omits_the_characters_crockford_omits() {
        let id =
            RequestId::from_parts(Id::<Request>::MAX_TIMESTAMP_MS, [0xff; 10]).expect("in range");
        for forbidden in ['I', 'L', 'O', 'U'] {
            assert!(
                !id.to_string().contains(forbidden),
                "{forbidden} must never appear"
            );
        }
        // Every alphabet character must decode back to its own index.
        for (index, &c) in ALPHABET.iter().enumerate() {
            assert_eq!(
                decode_char(c),
                Some(index as u8),
                "`{}` decodes wrong",
                c as char
            );
        }
    }

    #[test]
    fn parsing_fails_closed() {
        let good = RequestId::from_parts(1_785_000_131_000, ENTROPY)
            .expect("in range")
            .to_string();
        let body = good.strip_prefix("req_").expect("prefix");

        // Wrong kind: a receipt id is not a request id, and the type system is backed by the parser.
        assert!(ReceiptId::parse(&good).is_err());
        // Wrong prefix entirely, no prefix, empty.
        for bad in ["", "req", "req_", &format!("dlv_{body}"), body] {
            assert!(RequestId::parse(bad).is_err(), "`{bad}` must not parse");
        }
        // Ambiguous or lowercase characters.
        for bad in [
            "req_01K3M7QW8ZC4YRXB2N6VD9FTHI",
            "req_01k3m7qw8zc4yrxb2n6vd9fthe",
        ] {
            assert!(RequestId::parse(bad).is_err(), "`{bad}` must not parse");
        }
        // Too long, too short.
        assert!(RequestId::parse(&format!("req_{body}A")).is_err());
        assert!(RequestId::parse(&format!("req_{}", &body[..25])).is_err());
        // A leading character above `7` would need a 131st bit.
        assert!(RequestId::parse("req_81K3M7QW8ZC4YRXB2N6VD9FTHE").is_err());
    }

    #[test]
    fn a_timestamp_beyond_forty_eight_bits_is_refused() {
        assert!(RequestId::from_parts(Id::<Request>::MAX_TIMESTAMP_MS + 1, ENTROPY).is_err());
    }

    #[test]
    fn grant_handles_come_from_randomness_not_from_a_clock() {
        let handle = GrantHandle::from_random([0xff; 16]);
        let rendered = handle.to_string();
        assert!(rendered.starts_with("hg_"));
        assert_eq!(GrantHandle::parse(&rendered).expect("round trip"), handle);
        // Two different random inputs must not collide into one handle.
        let other = GrantHandle::from_random([0x01; 16]);
        assert_ne!(handle, other);
    }

    #[test]
    fn serde_uses_the_wire_string() {
        let id = RequestId::from_parts(1_785_000_131_000, ENTROPY).expect("in range");
        let json = serde_json::to_string(&id).expect("serialize");
        assert_eq!(json, format!("\"{id}\""));
        assert_eq!(
            serde_json::from_str::<RequestId>(&json).expect("deserialize"),
            id
        );
        assert!(
            serde_json::from_str::<ReceiptId>(&json).is_err(),
            "prefix is checked on the wire"
        );
    }

    #[test]
    fn machine_principals_are_distinguishable_by_type() {
        let person = PrincipalId::parse("usr_01J9ZP4KRTC4YRXB2N6VD9FTHE").expect("parse");
        let machine = PrincipalId::parse("sa_01J9ZP4KRTC4YRXB2N6VD9FTHE").expect("parse");
        let tenant = PrincipalId::parse("org_01K0A2XFV8C4YRXB2N6VD9FTHE").expect("parse");
        assert!(person.is_person() && !person.is_machine());
        assert!(machine.is_machine() && !machine.is_person());
        assert!(
            tenant.is_machine(),
            "a tenant acting as a principal is not a person either"
        );
        assert!(PrincipalId::parse("req_01K3M7QW8ZC4YRXB2N6VD9FTHE").is_err());
    }

    #[test]
    fn every_specified_prefix_is_present() {
        let mut prefixes = ALL_PREFIXES.to_vec();
        prefixes.sort_unstable();
        let mut expected = vec![
            "req", "rcpt", "auth", "dlv", "sig", "hg", "hs", "snk", "rt", "usr", "sa", "org",
        ];
        expected.sort_unstable();
        assert_eq!(
            prefixes, expected,
            "§1.4 lists exactly these prefixes, all REQUIRED"
        );
    }
}
