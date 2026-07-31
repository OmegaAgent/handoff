//! Why a delivery was withheld.
//!
//! The channel port itself is [`seam::DeliveryChannel`](crate::seam::DeliveryChannel) and the
//! directory behind it is [`seam::RecipientDirectory`](crate::seam::RecipientDirectory). There is
//! deliberately no second port here: two ways to describe a channel is one too many, and the
//! duplicate would be the exact per-item sprawl those traits exist to prevent.
//!
//! What this module adds is the missing vocabulary. §7.1 calls suppression "a real outcome, not a
//! failure" and requires it to stay visible, because an invisible suppression is indistinguishable
//! from a bug. A free-text detail string satisfies a human reading a log and nothing else — it
//! cannot be counted, alerted on, or tested against. So a suppression carries a **stable code**,
//! and the code is the part an operator's tooling reads.
//!
//! The reasons are worth enumerating rather than inventing per adapter, because most of them are
//! properties of the *deployment* rather than of a channel: nobody holds the role, the directory
//! has no address for this person, the ladder names a channel this build did not compile in. Those
//! are the honest answers to "why did nobody get paged", and each one names something an operator
//! can go and fix.

use crate::seam::DeliveryReport;

/// Why a delivery was withheld.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Suppression {
    /// The target resolved to nobody. An escalation rung that names no person is a broadcast into
    /// an empty room, and recording it is the only honest account of what happened (§7.5).
    NoRecipient,
    /// The directory knows the person but holds no address for them on this channel.
    NoAddress,
    /// No adapter is registered under this channel name. A ladder naming a channel this build did
    /// not compile in is a configuration error that must be visible per delivery, not a panic at
    /// startup that takes the deployment down over a rung nobody has reached yet.
    ChannelUnknown,
    /// The adapter is present but has no credentials, so it can send nothing. Said plainly rather
    /// than reported as a transport failure, which would invite a retry that could never succeed.
    NotConfigured {
        /// What an operator would have to supply. Recorded so the log says what is missing rather
        /// than only that something is.
        needs: String,
    },
    /// An identical live delivery already exists for this person on this channel.
    Duplicate,
    /// The person has said not to interrupt them now.
    QuietHours,
    /// The person has not consented to this channel.
    ConsentMissing,
    /// A reason this crate does not enumerate. The code travels verbatim.
    Other(String),
}

impl Suppression {
    /// The stable token recorded on the delivery and returned by the API.
    ///
    /// Stable in the sense §13 means for an error code: an operator may alert on it, so its
    /// meaning must not change within a major version.
    pub fn code(&self) -> &str {
        match self {
            Self::NoRecipient => "no_recipient",
            Self::NoAddress => "no_address",
            Self::ChannelUnknown => "channel_unknown",
            Self::NotConfigured { .. } => "channel_not_configured",
            Self::Duplicate => "duplicate",
            Self::QuietHours => "quiet_hours",
            Self::ConsentMissing => "consent_missing",
            Self::Other(code) => code,
        }
    }

    /// The sentence an operator reads next to the code.
    pub fn detail(&self) -> String {
        match self {
            Self::NoRecipient => "the target named nobody in this tenant".into(),
            Self::NoAddress => {
                "the directory holds no address for this person on this channel".into()
            }
            Self::ChannelUnknown => {
                "this build has no adapter registered under that channel name".into()
            }
            Self::NotConfigured { needs } => {
                format!("the adapter is not configured; it needs {needs}")
            }
            Self::Duplicate => "an identical live delivery already exists".into(),
            Self::QuietHours => "the person is within their quiet hours".into(),
            Self::ConsentMissing => "the person has not consented to this channel".into(),
            Self::Other(code) => format!("suppressed: {code}"),
        }
    }

    /// The report an adapter returns to withhold a delivery.
    pub fn report(&self) -> DeliveryReport {
        DeliveryReport::suppressed(format!("{}: {}", self.code(), self.detail()))
    }

    /// Recover the code from a report's detail, for a caller that has only the report.
    ///
    /// The code is everything before the first `": "`. Round-trips with [`Self::report`], and
    /// returns the whole string when a report was built by some other means — never a panic, and
    /// never a silent empty code.
    pub fn code_of(detail: &str) -> &str {
        detail.split_once(": ").map_or(detail, |(code, _)| code)
    }
}

impl std::fmt::Display for Suppression {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code(), self.detail())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use handoff_protocol::delivery::DeliveryState;

    #[test]
    fn every_reason_has_a_stable_token_and_a_sentence() {
        for reason in [
            Suppression::NoRecipient,
            Suppression::NoAddress,
            Suppression::ChannelUnknown,
            Suppression::NotConfigured {
                needs: "a sending credential".into(),
            },
            Suppression::Duplicate,
            Suppression::QuietHours,
            Suppression::ConsentMissing,
            Suppression::Other("tenant_paused".into()),
        ] {
            assert!(!reason.code().is_empty());
            assert!(!reason.code().contains(' '), "a code is one token");
            assert!(
                reason.detail().len() > reason.code().len(),
                "the detail must say more than the code"
            );
        }
    }

    #[test]
    fn a_suppression_round_trips_through_a_report() {
        let reason = Suppression::NotConfigured {
            needs: "a verified sending domain".into(),
        };
        let report = reason.report();
        assert_eq!(report.state, DeliveryState::Suppressed);
        let detail = report.detail.expect("a suppression says why");
        assert_eq!(Suppression::code_of(&detail), "channel_not_configured");
        assert!(detail.contains("a verified sending domain"));
    }

    #[test]
    fn a_detail_from_somewhere_else_still_yields_something_rather_than_nothing() {
        assert_eq!(Suppression::code_of("quiet_hours"), "quiet_hours");
        assert_eq!(Suppression::code_of(""), "");
    }
}
