//! Minting identifiers.
//!
//! Two constructors, and the difference between them is a security property rather than a
//! convenience. Protocol identifiers are ULIDs: time-sortable, safe to log, and ordinary data
//! (§1.4). Capability grant handles are **not** — §11.1 requires them to come from a
//! cryptographically secure random source and forbids deriving them from a resource name or a
//! shared secret, because a derived handle cannot be rotated without rotating every resource at
//! once and cannot be revoked individually at all.

use handoff_protocol::error::{ErrorCode, ProtocolError, Result};
use handoff_protocol::id::{Id, IdKind};

/// Fill a buffer from the platform CSPRNG.
fn random_bytes<const N: usize>() -> Result<[u8; N]> {
    let mut buf = [0u8; N];
    getrandom::getrandom(&mut buf).map_err(|e| {
        ProtocolError::new(
            ErrorCode::InvalidRequest,
            format!("the system random source is unavailable: {e}"),
        )
    })?;
    Ok(buf)
}

/// Mint a time-sortable identifier for `now`, with 80 bits of entropy.
pub fn mint<K: IdKind>(now_ms: u64) -> Result<Id<K>> {
    Id::from_parts(now_ms, random_bytes::<10>()?)
}

/// Mint an unguessable identifier with no time component.
///
/// This is the constructor for a capability grant handle (§11.1) and for anything else that is a
/// bearer-adjacent secret rather than a name.
pub fn mint_random<K: IdKind>() -> Result<Id<K>> {
    Ok(Id::from_random(random_bytes::<16>()?))
}

/// A URL-safe random string, for values that are never stored and never named by the protocol.
pub fn random_token() -> Result<String> {
    const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    let bytes = random_bytes::<20>()?;
    Ok(bytes
        .iter()
        .map(|b| ALPHABET[(b & 0x1f) as usize] as char)
        .collect())
}
