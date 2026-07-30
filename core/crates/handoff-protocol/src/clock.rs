//! Time, injected.
//!
//! This crate never reads the wall clock. `chrono` is compiled without its `clock` feature, so
//! `Utc::now()` does not exist here — the compiler enforces what a review comment otherwise would.
//! "Now" arrives through the [`Clock`] port, and a real system clock is `handoff-core`'s to provide.
//!
//! Two time types are defined here because the protocol uses two (§1.4): [`Timestamp`], an RFC 3339
//! instant on the Server's own clock, and [`IsoDuration`], an ISO 8601 duration such as `PT15M`.

use crate::error::{ErrorCode, ProtocolError, Result};
use chrono::{DateTime, SecondsFormat, Utc};
use serde::{de, Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::sync::{Arc, Mutex};

/// A source of "now".
///
/// Every recorded time in the protocol comes from the Server's own clock; a Server MUST NOT accept
/// a client-supplied `decided_at` (§1.4). Making that a port rather than a call to the operating
/// system is what lets the conformance suite advance time without sleeping.
pub trait Clock: Send + Sync {
    /// The current instant, on the Server's clock.
    fn now(&self) -> Timestamp;
}

impl<T: Clock + ?Sized> Clock for Arc<T> {
    fn now(&self) -> Timestamp {
        (**self).now()
    }
}

/// A clock frozen at one instant.
///
/// Useful where a whole transaction must record a single consistent time — which the protocol
/// requires anyway, since a receipt and its state change are minted together (§9.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FixedClock(Timestamp);

impl FixedClock {
    /// A clock that always reports `at`.
    pub const fn new(at: Timestamp) -> Self {
        Self(at)
    }
}

impl Clock for FixedClock {
    fn now(&self) -> Timestamp {
        self.0
    }
}

/// A manually advanced clock, for tests that need time to pass without waiting for it.
#[derive(Debug, Clone)]
pub struct ManualClock {
    inner: Arc<Mutex<Timestamp>>,
}

impl ManualClock {
    /// A clock starting at `start`.
    pub fn new(start: Timestamp) -> Self {
        Self {
            inner: Arc::new(Mutex::new(start)),
        }
    }

    /// Move the clock forward. Panics if the lock was poisoned by a panicking test.
    pub fn advance(&self, by: IsoDuration) {
        let mut guard = self.inner.lock().expect("ManualClock lock poisoned");
        *guard = guard.saturating_add(by);
    }

    /// Move the clock to an absolute instant.
    pub fn set(&self, to: Timestamp) {
        *self.inner.lock().expect("ManualClock lock poisoned") = to;
    }
}

impl Clock for ManualClock {
    fn now(&self) -> Timestamp {
        *self.inner.lock().expect("ManualClock lock poisoned")
    }
}

/// An RFC 3339 instant in UTC, as every timestamp in this protocol is (§1.4).
///
/// Serializes as `2026-07-30T14:02:11Z`: `Z`, never `+00:00`, and sub-second digits only when the
/// value actually carries them. That matters because timestamps land inside receipts, and receipts
/// are hashed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Timestamp(DateTime<Utc>);

impl Timestamp {
    /// Wrap a `chrono` instant.
    pub const fn from_datetime(at: DateTime<Utc>) -> Self {
        Self(at)
    }

    /// The instant as a `chrono` value.
    pub const fn to_datetime(self) -> DateTime<Utc> {
        self.0
    }

    /// Build from milliseconds since the Unix epoch.
    ///
    /// Returns `None` for a value outside the representable range.
    pub fn from_millis(millis: i64) -> Option<Self> {
        DateTime::<Utc>::from_timestamp_millis(millis).map(Self)
    }

    /// Milliseconds since the Unix epoch. This is what an identifier's time component carries.
    pub fn to_millis(self) -> i64 {
        self.0.timestamp_millis()
    }

    /// Parse an RFC 3339 timestamp, normalizing any offset to UTC.
    pub fn parse(s: &str) -> Result<Self> {
        DateTime::parse_from_rfc3339(s)
            .map(|dt| Self(dt.with_timezone(&Utc)))
            .map_err(|e| {
                ProtocolError::new(
                    ErrorCode::InvalidRequest,
                    format!("not an RFC 3339 timestamp: {e}"),
                )
            })
    }

    /// This instant plus `duration`, saturating at the representable range rather than wrapping.
    ///
    /// Saturation is the safe direction for a deadline: an unrepresentably distant `expires_at`
    /// means "effectively never expires", where wrapping would mean "expired long ago".
    #[must_use]
    pub fn saturating_add(self, duration: IsoDuration) -> Self {
        match chrono::TimeDelta::try_seconds(duration.as_secs() as i64)
            .and_then(|d| self.0.checked_add_signed(d))
        {
            Some(at) => Self(at),
            None => Self(DateTime::<Utc>::MAX_UTC),
        }
    }

    /// Whether this instant is at or before `other` — the shape every deadline guard uses
    /// (`expires_at <= now`).
    pub fn is_at_or_before(self, other: Timestamp) -> bool {
        self.0 <= other.0
    }
}

impl fmt::Display for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0.to_rfc3339_opts(SecondsFormat::AutoSi, true))
    }
}

impl Serialize for Timestamp {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Timestamp {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Timestamp::parse(&raw).map_err(de::Error::custom)
    }
}

/// An ISO 8601 duration, in the subset the protocol actually uses: `PT15M`, `PT4H`, `P1D`, `PT0S`.
///
/// Years and months are **rejected**, not approximated. Their length depends on when you start, and
/// a deadline that means something different in February is not a deadline. Weeks are accepted as
/// exactly seven days.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct IsoDuration {
    secs: u64,
}

impl IsoDuration {
    /// A zero-length duration, `PT0S`.
    pub const ZERO: Self = Self { secs: 0 };

    /// Build from whole seconds.
    pub const fn from_secs(secs: u64) -> Self {
        Self { secs }
    }

    /// Build from whole minutes.
    pub const fn from_mins(mins: u64) -> Self {
        Self { secs: mins * 60 }
    }

    /// Build from whole hours.
    pub const fn from_hours(hours: u64) -> Self {
        Self { secs: hours * 3600 }
    }

    /// The duration in whole seconds.
    pub const fn as_secs(self) -> u64 {
        self.secs
    }

    /// Parse an ISO 8601 duration.
    ///
    /// Fails closed: an unparseable or unsupported designator is an error, never a silently
    /// substituted default. A duration nobody can interpret is a deadline nobody can honour.
    pub fn parse(s: &str) -> Result<Self> {
        fn bad(s: &str, why: &str) -> ProtocolError {
            ProtocolError::new(
                ErrorCode::InvalidRequest,
                format!("`{s}` is not a supported ISO 8601 duration: {why}"),
            )
        }

        let rest = s
            .strip_prefix('P')
            .ok_or_else(|| bad(s, "must start with `P`"))?;
        let (date_part, time_part) = match rest.split_once('T') {
            Some((d, t)) => {
                if t.is_empty() {
                    return Err(bad(s, "`T` must be followed by a time component"));
                }
                (d, Some(t))
            }
            None => (rest, None),
        };
        if date_part.is_empty() && time_part.is_none() {
            return Err(bad(s, "no components"));
        }

        let mut secs: u64 = 0;
        let mut digits = String::new();
        let mut saw_component = false;

        let consume = |unit: char, digits: &mut String, secs: &mut u64| -> Result<()> {
            if digits.is_empty() {
                return Err(bad(s, "a designator with no number"));
            }
            let n: u64 = digits
                .parse()
                .map_err(|_| bad(s, "component out of range"))?;
            digits.clear();
            let mult = match unit {
                'D' => 86_400,
                'W' => 604_800,
                'H' => 3_600,
                'M' => 60,
                'S' => 1,
                _ => return Err(bad(s, "unsupported designator")),
            };
            *secs = secs
                .checked_add(n.checked_mul(mult).ok_or_else(|| bad(s, "overflow"))?)
                .ok_or_else(|| bad(s, "overflow"))?;
            Ok(())
        };

        for ch in date_part.chars() {
            match ch {
                '0'..='9' => digits.push(ch),
                'D' | 'W' => {
                    consume(ch, &mut digits, &mut secs)?;
                    saw_component = true;
                }
                'Y' | 'M' => {
                    return Err(bad(
                        s,
                        "years and months have no fixed length and are rejected",
                    ))
                }
                _ => return Err(bad(s, "unexpected character in the date component")),
            }
        }
        if !digits.is_empty() {
            return Err(bad(s, "trailing number with no designator"));
        }

        if let Some(time_part) = time_part {
            for ch in time_part.chars() {
                match ch {
                    '0'..='9' => digits.push(ch),
                    'H' | 'M' | 'S' => {
                        consume(ch, &mut digits, &mut secs)?;
                        saw_component = true;
                    }
                    _ => return Err(bad(s, "unexpected character in the time component")),
                }
            }
            if !digits.is_empty() {
                return Err(bad(s, "trailing number with no designator"));
            }
        }

        if !saw_component {
            return Err(bad(s, "no components"));
        }
        Ok(Self { secs })
    }
}

impl fmt::Display for IsoDuration {
    /// Renders canonically: whole days first, then `T` and the time components, and `PT0S` for zero.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.secs == 0 {
            return f.write_str("PT0S");
        }
        let days = self.secs / 86_400;
        let rem = self.secs % 86_400;
        let (hours, mins, secs) = (rem / 3600, (rem % 3600) / 60, rem % 60);
        f.write_str("P")?;
        if days > 0 {
            write!(f, "{days}D")?;
        }
        if hours > 0 || mins > 0 || secs > 0 {
            f.write_str("T")?;
            if hours > 0 {
                write!(f, "{hours}H")?;
            }
            if mins > 0 {
                write!(f, "{mins}M")?;
            }
            if secs > 0 {
                write!(f, "{secs}S")?;
            }
        }
        Ok(())
    }
}

impl Serialize for IsoDuration {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for IsoDuration {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        IsoDuration::parse(&raw).map_err(de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts(s: &str) -> Timestamp {
        Timestamp::parse(s).expect("valid timestamp")
    }

    #[test]
    fn timestamps_render_the_way_the_spec_writes_them() {
        assert_eq!(
            ts("2026-07-30T14:02:11Z").to_string(),
            "2026-07-30T14:02:11Z"
        );
        // An offset is normalized to UTC rather than preserved, so two equal instants hash equally.
        assert_eq!(
            ts("2026-07-30T16:02:11+02:00").to_string(),
            "2026-07-30T14:02:11Z"
        );
    }

    #[test]
    fn timestamp_serde_round_trips() {
        let at = ts("2026-07-30T14:07:44Z");
        let json = serde_json::to_string(&at).expect("serialize");
        assert_eq!(json, "\"2026-07-30T14:07:44Z\"");
        assert_eq!(
            serde_json::from_str::<Timestamp>(&json).expect("deserialize"),
            at
        );
    }

    #[test]
    fn durations_parse_the_forms_the_spec_uses() {
        assert_eq!(IsoDuration::parse("PT15M").expect("parse").as_secs(), 900);
        assert_eq!(IsoDuration::parse("PT4H").expect("parse").as_secs(), 14_400);
        assert_eq!(IsoDuration::parse("P1D").expect("parse").as_secs(), 86_400);
        assert_eq!(IsoDuration::parse("PT0S").expect("parse").as_secs(), 0);
        assert_eq!(
            IsoDuration::parse("P1DT2H3M4S").expect("parse").as_secs(),
            93_784
        );
        assert_eq!(IsoDuration::parse("P1W").expect("parse").as_secs(), 604_800);
    }

    #[test]
    fn durations_fail_closed_on_anything_ambiguous_or_malformed() {
        for bad in [
            "P1Y", "P1M", "PT15", "15M", "P", "PT", "P1DT", "PT1X", "PT-5M", "",
        ] {
            assert!(
                IsoDuration::parse(bad).is_err(),
                "`{bad}` must not parse: an uninterpretable duration is a deadline nobody can honour"
            );
        }
    }

    #[test]
    fn duration_display_round_trips_through_parse() {
        for secs in [0, 1, 59, 60, 900, 3_600, 86_400, 93_784, 604_800] {
            let d = IsoDuration::from_secs(secs);
            let rendered = d.to_string();
            assert_eq!(
                IsoDuration::parse(&rendered).expect("re-parse"),
                d,
                "{rendered}"
            );
        }
        assert_eq!(IsoDuration::from_mins(15).to_string(), "PT15M");
        assert_eq!(IsoDuration::from_hours(4).to_string(), "PT4H");
        assert_eq!(IsoDuration::from_secs(86_400).to_string(), "P1D");
    }

    #[test]
    fn attempt_deadline_arithmetic_uses_the_injected_clock() {
        let clock = ManualClock::new(ts("2026-07-30T14:02:11Z"));
        let deadline = clock.now().saturating_add(IsoDuration::from_mins(15));
        assert_eq!(deadline.to_string(), "2026-07-30T14:17:11Z");
        assert!(!deadline.is_at_or_before(clock.now()));
        clock.advance(IsoDuration::from_mins(15));
        assert!(
            deadline.is_at_or_before(clock.now()),
            "the attempt has lapsed at exactly its deadline"
        );
    }

    #[test]
    fn adding_an_unrepresentable_duration_saturates_instead_of_wrapping() {
        let far = ts("2026-07-30T14:02:11Z").saturating_add(IsoDuration::from_secs(u64::MAX / 2));
        assert!(!far.is_at_or_before(ts("9999-01-01T00:00:00Z")));
    }
}
