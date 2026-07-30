//! The numbered invariant registry of spec §17, and its mapping to the conformance suite of §18.
//!
//! **These numbers are stable.** The conformance suite references them and they MUST NOT be
//! renumbered; additions take the next free number (§17). The registry is iterable so that a test
//! can walk it, which is the point: the meta-test in this module asserts every invariant has at
//! least one conformance case mapped to it, and fails loudly when one does not.
//!
//! That test is what stops coverage drifting. An invariant nobody tests is a sentence in a
//! document, and a specification whose guarantees are only sentences is the thing this protocol
//! exists to replace.
//!
//! # Where the mapping comes from
//!
//! Every entry records its provenance in [`MappingSource`]:
//!
//! * [`MappingSource::Section18Table`] — the "Proves" column of §18's table, verbatim.
//! * [`MappingSource::SectionCrossReference`] — a `Conformance:` line in the normative body that
//!   names a case for an invariant §18's table does not list against it.
//! * [`MappingSource::Proposed`] — **no case exists anywhere in the specification.** These are the
//!   coverage gaps this crate found; see [`INVARIANTS_WITH_NO_SPECIFIED_CASE`] and the crate
//!   documentation's spec defect D-1. The identifiers are deliberately not `C-` numbers, so they
//!   cannot collide with whatever the specification eventually assigns.

use std::fmt;

/// A stable invariant number, `I1` through `I21`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InvariantId(u8);

impl InvariantId {
    /// The bare number.
    pub const fn number(self) -> u8 {
        self.0
    }
}

impl fmt::Display for InvariantId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "I{}", self.0)
    }
}

/// Where a mapping between an invariant and a conformance case came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MappingSource {
    /// The "Proves" column of §18's conformance table.
    Section18Table,
    /// A `Conformance:` line in the normative body, naming the section it appears in.
    SectionCrossReference(&'static str),
    /// No case exists in the specification. This crate proposes one so that coverage is visible
    /// rather than silently absent; the specification's owner must ratify it.
    Proposed,
}

/// One conformance case mapped to an invariant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ConformanceCase {
    /// The case identifier, for example `C-3`.
    pub id: &'static str,
    /// Where this mapping came from.
    pub source: MappingSource,
}

impl ConformanceCase {
    const fn spec(id: &'static str) -> Self {
        Self {
            id,
            source: MappingSource::Section18Table,
        }
    }
    const fn cross_referenced(id: &'static str, section: &'static str) -> Self {
        Self {
            id,
            source: MappingSource::SectionCrossReference(section),
        }
    }
    const fn proposed(id: &'static str) -> Self {
        Self {
            id,
            source: MappingSource::Proposed,
        }
    }

    /// Whether this mapping is backed by the specification at all.
    pub const fn is_specified(self) -> bool {
        !matches!(self.source, MappingSource::Proposed)
    }
}

/// One numbered invariant and the cases that prove it.
#[derive(Debug, Clone, Copy)]
pub struct Invariant {
    /// The stable number.
    pub id: InvariantId,
    /// The invariant, quoted from §17.
    pub text: &'static str,
    /// The conformance cases mapped to it.
    pub cases: &'static [ConformanceCase],
}

const fn invariant(number: u8, text: &'static str, cases: &'static [ConformanceCase]) -> Invariant {
    Invariant {
        id: InvariantId(number),
        text,
        cases,
    }
}

/// The twenty-one invariants of §17, in order.
pub const INVARIANTS: &[Invariant] = &[
    invariant(
        1,
        "One request has many deliveries and at most one decision receipt.",
        &[ConformanceCase::spec("C-2")],
    ),
    invariant(
        2,
        "A receipt is immutable and records what the person saw, not what the request later became.",
        &[ConformanceCase::spec("C-15")],
    ),
    invariant(
        3,
        "Escalation, reminders, and channel fallback mint deliveries, never requests.",
        &[ConformanceCase::spec("C-14")],
    ),
    invariant(
        4,
        "A pending request is always listable and always answerable at its canonical URL. A lapsed \
         attempt changes urgency, never visibility.",
        &[ConformanceCase::spec("C-9")],
    ),
    invariant(
        5,
        "Answering is first-writer-wins. A conflicting second answer is a 409 and changes nothing. \
         A landed answer beats a racing cancel or expiry.",
        &[ConformanceCase::spec("C-3"), ConformanceCase::spec("C-4")],
    ),
    invariant(
        6,
        "Required authority is declared on the request and evaluated at answer time against the \
         answerer's authenticated identity. A delivery channel never confers authority.",
        &[ConformanceCase::spec("C-6"), ConformanceCase::spec("C-21")],
    ),
    invariant(
        7,
        "`secret` values never enter the request, the receipt, the event record, a waiter signal, \
         or any delivery. Only {\"provided\": true} travels.",
        &[ConformanceCase::spec("C-7")],
    ),
    invariant(
        8,
        "Capability grants are opaque handles resolved through an authenticated endpoint. The \
         protocol never carries a resolvable address by value.",
        &[ConformanceCase::spec("C-8")],
    ),
    invariant(
        9,
        "The outcome is delivered to the waiter as typed data, retried until acked; the ack is \
         idempotent.",
        &[ConformanceCase::spec("C-12")],
    ),
    invariant(
        10,
        "One answer mints one authorization. Redemption is idempotent per `effect_key`, and a \
         single-use authorization cannot be spent twice.",
        &[ConformanceCase::spec("C-13")],
    ),
    invariant(
        11,
        "Every terminal transition produces a typed terminal signal. A request never goes quiet.",
        &[ConformanceCase::spec("C-10")],
    ),
    invariant(
        12,
        "Every state transition emits its event in the same transaction as the state change.",
        // §18 maps no case to I12, and no `Conformance:` line elsewhere names one. See D-1.
        &[ConformanceCase::proposed("PROPOSED-I12")],
    ),
    invariant(
        13,
        "Tenant binding is resolved from stored state, never from a request body.",
        // §18's table does not list I13, but §4.7 — which states I13 — ends "Conformance: C-21."
        &[ConformanceCase::cross_referenced("C-21", "§4.7")],
    ),
    invariant(
        14,
        "The core never switches on a request kind. New interaction types arrive as new field types \
         or capability types, behind the declaration.",
        &[ConformanceCase::spec("C-22")],
    ),
    invariant(
        15,
        "A requester principal can never answer its own request.",
        &[ConformanceCase::spec("C-5")],
    ),
    invariant(
        16,
        "A decision originates from an authenticated principal. It is never derived from message \
         content, and never inferred from observed state.",
        &[ConformanceCase::spec("C-21"), ConformanceCase::spec("C-22")],
    ),
    invariant(
        17,
        "Every identifier and every client-supplied key is tenant-scoped. Lookups are \
         tenant-scoped; uniqueness is tenant-scoped; possession of an identifier is never \
         authorization.",
        &[ConformanceCase::spec("C-19"), ConformanceCase::spec("C-20")],
    ),
    invariant(
        18,
        "Secret values never appear in a URL, query string, path, argv, environment variable, \
         header, redirect, log line, metric label, trace attribute, or crash report.",
        &[ConformanceCase::spec("C-7")],
    ),
    invariant(
        19,
        "A capability grant declares its blast radius; the person is shown it before accepting; the \
         accepted digest binds the resolve; the receipt records its digest.",
        // §18 maps no case to I19. C-8 covers grant opacity and replay, not blast radius. See D-1.
        &[ConformanceCase::proposed("PROPOSED-I19")],
    ),
    invariant(
        20,
        "Every mutating operation is idempotent under a caller-supplied key, and every idempotency \
         key is tenant-scoped.",
        &[ConformanceCase::spec("C-1")],
    ),
    invariant(
        21,
        "Unknown protocol versions, field types, and capability types fail closed. Nothing is \
         created, and nothing degrades silently.",
        &[ConformanceCase::spec("C-16")],
    ),
];

/// The invariants for which the specification names **no** conformance case anywhere.
///
/// This list is asserted exactly by a test, so it cannot grow unnoticed and cannot silently persist
/// once the specification closes the gap. It is reported as spec defect D-1 rather than fixed here:
/// this crate does not own `spec/`.
pub const INVARIANTS_WITH_NO_SPECIFIED_CASE: &[u8] = &[12, 19];

/// Every conformance case §18 enumerates.
///
/// Level 1 is C-1 through C-16 plus C-6b, C-18, C-19, C-20, C-21, and C-22. Level 2 adds C-17.
pub const SECTION_18_CASES: &[&str] = &[
    "C-1", "C-2", "C-3", "C-4", "C-5", "C-6", "C-6b", "C-7", "C-8", "C-9", "C-10", "C-11", "C-12",
    "C-13", "C-14", "C-15", "C-16", "C-17", "C-18", "C-19", "C-20", "C-21", "C-22",
];

/// Cases §18 lists whose "Proves" column names a section rather than a numbered invariant.
///
/// They are not gaps: each proves a normative requirement that §17 does not restate as a numbered
/// invariant. Listing them explicitly is what lets the coverage test assert the §18 table in both
/// directions.
pub const CASES_PROVING_A_SECTION_NOT_AN_INVARIANT: &[(&str, &str)] = &[
    (
        "C-6b",
        "§4.4 — `link_only` is refused unless the deployment opted in",
    ),
    (
        "C-11",
        "W7 — reattachment returns an unacked signal after a client restart",
    ),
    ("C-17", "§14 — the Level 2 `continuation` extension"),
    (
        "C-18",
        "§15 — callback signing, replay rejection, and per-waiter sequence monotonicity",
    ),
];

/// Look one invariant up by number.
pub fn get(number: u8) -> Option<&'static Invariant> {
    INVARIANTS.iter().find(|i| i.id.number() == number)
}

/// Every case mapped to an invariant, in registry order, deduplicated.
pub fn mapped_cases() -> Vec<&'static str> {
    let mut cases: Vec<&'static str> = INVARIANTS
        .iter()
        .flat_map(|i| i.cases.iter().map(|c| c.id))
        .collect();
    cases.sort_unstable();
    cases.dedup();
    cases
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// **The meta-test.** Every invariant I1..I21 must have at least one conformance case mapped to
    /// it. A new invariant added without a case fails here, loudly, at the moment it is added.
    #[test]
    fn every_invariant_has_at_least_one_conformance_case() {
        let uncovered: Vec<String> = INVARIANTS
            .iter()
            .filter(|i| i.cases.is_empty())
            .map(|i| format!("{} — {}", i.id, i.text))
            .collect();
        assert!(
            uncovered.is_empty(),
            "these invariants have no conformance case, so nothing stops them regressing:\n  {}",
            uncovered.join("\n  ")
        );
    }

    /// The gap, pinned. This fails when the specification closes it (remove the proposal) and when
    /// a new gap appears (add a case, or a proposal, and update this list deliberately).
    #[test]
    fn the_invariants_with_no_specified_case_are_exactly_the_known_gap() {
        let actual: Vec<u8> = INVARIANTS
            .iter()
            .filter(|i| !i.cases.iter().any(|c| c.is_specified()))
            .map(|i| i.id.number())
            .collect();
        assert_eq!(
            actual, INVARIANTS_WITH_NO_SPECIFIED_CASE,
            "the set of invariants the specification does not test has changed; update \
             INVARIANTS_WITH_NO_SPECIFIED_CASE deliberately, and report or close the defect"
        );
    }

    #[test]
    fn the_registry_is_i1_through_i21_and_nothing_is_renumbered() {
        // §17: these numbers are stable, and the conformance suite references them.
        let numbers: Vec<u8> = INVARIANTS.iter().map(|i| i.id.number()).collect();
        assert_eq!(numbers, (1..=21).collect::<Vec<u8>>());
        assert_eq!(INVARIANTS.len(), 21);
        for i in INVARIANTS {
            assert!(!i.text.is_empty(), "{} has no text", i.id);
            assert_eq!(get(i.id.number()).map(|f| f.id), Some(i.id));
        }
        assert!(get(0).is_none() && get(22).is_none());
    }

    #[test]
    fn every_specified_case_is_one_of_section_eighteens() {
        let known: BTreeSet<&str> = SECTION_18_CASES.iter().copied().collect();
        for invariant in INVARIANTS {
            for case in invariant.cases {
                if case.is_specified() {
                    assert!(
                        known.contains(case.id),
                        "{} maps to `{}`, which §18 does not define",
                        invariant.id,
                        case.id
                    );
                }
            }
        }
    }

    #[test]
    fn every_section_eighteen_case_is_accounted_for() {
        // The other direction: a case that proves nothing in the registry and is not on the
        // "proves a section" list means the mapping table has drifted from §18.
        let mapped: BTreeSet<&str> = mapped_cases().into_iter().collect();
        let section_cases: BTreeSet<&str> = CASES_PROVING_A_SECTION_NOT_AN_INVARIANT
            .iter()
            .map(|(id, _)| *id)
            .collect();
        let unaccounted: Vec<&str> = SECTION_18_CASES
            .iter()
            .copied()
            .filter(|id| !mapped.contains(id) && !section_cases.contains(id))
            .collect();
        assert!(
            unaccounted.is_empty(),
            "these §18 cases are mapped to nothing: {unaccounted:?}"
        );
        // And nothing is on both lists, which would mean a case is claimed twice.
        for (id, _) in CASES_PROVING_A_SECTION_NOT_AN_INVARIANT {
            assert!(
                !mapped.contains(id),
                "`{id}` is listed as proving both a section and an invariant"
            );
        }
    }

    #[test]
    fn proposed_cases_cannot_be_mistaken_for_specified_ones() {
        for invariant in INVARIANTS {
            for case in invariant.cases {
                if case.source == MappingSource::Proposed {
                    assert!(
                        case.id.starts_with("PROPOSED-"),
                        "`{}` is proposed but is spelled like a specified case id",
                        case.id
                    );
                }
            }
        }
    }

    #[test]
    fn cross_referenced_mappings_name_the_section_they_came_from() {
        let mut found = 0;
        for invariant in INVARIANTS {
            for case in invariant.cases {
                if let MappingSource::SectionCrossReference(section) = case.source {
                    assert!(
                        section.starts_with('§'),
                        "{section} should name a spec section"
                    );
                    found += 1;
                }
            }
        }
        // I13 is the one invariant covered by a `Conformance:` line rather than by §18's table.
        assert_eq!(found, 1);
    }
}
