//! The numbered invariant registry of spec §17.
//!
//! **These numbers are stable.** The conformance suite references them and they MUST NOT be
//! renumbered; additions take the next free number (§17). The registry is iterable so that a test
//! can walk it, which is the point: the meta-test in this module asserts every invariant has at
//! least one conformance case proving it, and fails loudly when one does not.
//!
//! That test is what stops coverage drifting. An invariant nobody tests is a sentence in a
//! document, and a specification whose guarantees are only sentences is the thing this protocol
//! exists to replace.
//!
//! # Where the mapping comes from, and why not from here
//!
//! This module deliberately holds **only the numbers and their text**. It does not hold a copy of
//! the invariant-to-case mapping, because a copy is exactly the thing that drifts: a hardcoded
//! table keeps passing after the suite it claims to describe has changed underneath it.
//!
//! Coverage is derived at test time from **the conformance case files themselves**
//! (`conformance/cases/*.yaml`, each declaring `id:`, `level:` and `invariants:`). Those files are
//! the executable suite, and a case that does not exist cannot prove anything whatever a table says
//! about it. A second test cross-checks them against `spec/conformance-map.json` and fails on any
//! disagreement, so drift between the suite and the normative mapping is caught in both directions.
//!
//! Reading files is confined to `#[cfg(test)]`. The library itself performs no I/O (see the crate
//! documentation), and `cargo publish` verifies with a build rather than a test run, so this does
//! not affect the published crate.

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

/// One numbered invariant, quoted from §17.
#[derive(Debug, Clone, Copy)]
pub struct Invariant {
    /// The stable number.
    pub id: InvariantId,
    /// The invariant, quoted from §17.
    pub text: &'static str,
}

const fn invariant(number: u8, text: &'static str) -> Invariant {
    Invariant {
        id: InvariantId(number),
        text,
    }
}

/// The twenty-one invariants of §17, in order.
pub const INVARIANTS: &[Invariant] = &[
    invariant(
        1,
        "One request has many deliveries and at most one decision receipt.",
    ),
    invariant(
        2,
        "A receipt is immutable and records what the person saw, not what the request later became.",
    ),
    invariant(
        3,
        "Escalation, reminders, and channel fallback mint deliveries, never requests.",
    ),
    invariant(
        4,
        "A pending request is always listable and always answerable at its canonical URL. A lapsed \
         attempt changes urgency, never visibility.",
    ),
    invariant(
        5,
        "Answering is first-writer-wins. A conflicting second answer is a 409 and changes nothing. \
         A landed answer beats a racing cancel or expiry.",
    ),
    invariant(
        6,
        "Required authority is declared on the request and evaluated at answer time against the \
         answerer's authenticated identity. A delivery channel never confers authority.",
    ),
    invariant(
        7,
        "`secret` values never enter the request, the receipt, the event record, a waiter signal, \
         or any delivery. Only {\"provided\": true} travels.",
    ),
    invariant(
        8,
        "Capability grants are opaque handles resolved through an authenticated endpoint. The \
         protocol never carries a resolvable address by value.",
    ),
    invariant(
        9,
        "The outcome is delivered to the waiter as typed data, retried until acked; the ack is \
         idempotent.",
    ),
    invariant(
        10,
        "One answer mints one authorization. Redemption is idempotent per `effect_key`, and a \
         single-use authorization cannot be spent twice.",
    ),
    invariant(
        11,
        "Every terminal transition produces a typed terminal signal. A request never goes quiet.",
    ),
    invariant(
        12,
        "Every state transition emits its event in the same transaction as the state change.",
    ),
    invariant(
        13,
        "Tenant binding is resolved from stored state, never from a request body.",
    ),
    invariant(
        14,
        "The core never switches on a request kind. New interaction types arrive as new field types \
         or capability types, behind the declaration.",
    ),
    invariant(15, "A requester principal can never answer its own request."),
    invariant(
        16,
        "A decision originates from an authenticated principal. It is never derived from message \
         content, and never inferred from observed state. Inference may be recorded as \
         `runtime_inference` with no actor; it is never recorded as a person.",
    ),
    invariant(
        17,
        "Every identifier and every client-supplied key is tenant-scoped. Lookups are \
         tenant-scoped; uniqueness is tenant-scoped; possession of an identifier is never \
         authorization.",
    ),
    invariant(
        18,
        "Secret values never appear in a URL, query string, path, argv, environment variable, \
         header, redirect, log line, metric label, trace attribute, or crash report.",
    ),
    invariant(
        19,
        "A capability grant declares its blast radius; the person is shown it before accepting; the \
         accepted digest binds the resolve; the receipt records its digest.",
    ),
    invariant(
        20,
        "Every mutating operation is idempotent under a caller-supplied key, and every idempotency \
         key is tenant-scoped.",
    ),
    invariant(
        21,
        "Unknown protocol versions, field types, and capability types fail closed. Nothing is \
         created, and nothing degrades silently.",
    ),
];

/// Look one invariant up by number.
pub fn get(number: u8) -> Option<&'static Invariant> {
    INVARIANTS.iter().find(|i| i.id.number() == number)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_registry_is_i1_through_i21_and_nothing_is_renumbered() {
        // §17: these numbers are stable, and the conformance suite references them.
        let numbers: Vec<u8> = INVARIANTS.iter().map(|i| i.id.number()).collect();
        assert_eq!(numbers, (1..=21).collect::<Vec<u8>>());
        for i in INVARIANTS {
            assert!(!i.text.is_empty(), "{} has no text", i.id);
            assert_eq!(get(i.id.number()).map(|f| f.id), Some(i.id));
        }
        assert!(get(0).is_none() && get(22).is_none());
    }
}

/// Coverage, derived from the conformance suite on disk rather than from a copy of it.
#[cfg(test)]
mod coverage {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet};
    use std::path::{Path, PathBuf};

    /// One case, as the suite declares it.
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct Case {
        id: String,
        level: u8,
        invariants: Vec<String>,
        withdrawn: bool,
    }

    fn repo_root() -> PathBuf {
        // `<root>/core/crates/handoff-protocol` → `<root>`.
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(3)
            .expect("the crate sits three levels below the repository root")
            .to_path_buf()
    }

    /// Read every case file and parse the fields coverage depends on.
    ///
    /// Deliberately strict. A file that does not declare `id:`, `level:` and `invariants:` is a
    /// failure, not a file to skip — silently skipping an unparseable case is how a suite loses a
    /// case and still reports green.
    fn cases_from_files() -> Vec<Case> {
        let dir = repo_root().join("conformance/cases");
        let entries = std::fs::read_dir(&dir).unwrap_or_else(|e| {
            panic!(
                "cannot read the conformance suite at {}: {e}. Coverage is derived from the case \
                 files; without them this test proves nothing and must not pass.",
                dir.display()
            )
        });

        let mut cases = Vec::new();
        for entry in entries {
            let path = entry.expect("directory entry").path();
            if path.extension().and_then(|e| e.to_str()) != Some("yaml") {
                continue;
            }
            let text = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
            cases.push(parse_case(&text, &path));
        }
        assert!(
            !cases.is_empty(),
            "no case files found in {}",
            dir.display()
        );
        cases.sort_by(|a, b| a.id.cmp(&b.id));
        cases
    }

    /// A deliberately narrow reader for the top-level keys this test needs.
    ///
    /// The case format declares `id:`, `level:`, `invariants:` and optionally `withdrawn:` as
    /// top-level scalars or inline lists, so a full YAML parser would be a dependency bought for
    /// four lines of grammar. It panics rather than guessing on anything it cannot read.
    fn parse_case(text: &str, path: &Path) -> Case {
        let mut id = None;
        let mut level = None;
        let mut invariants = None;
        let mut withdrawn = false;

        for line in text.lines() {
            // Top-level keys only: an indented `id:` belongs to a step, not to the case.
            if line.starts_with(char::is_whitespace) {
                continue;
            }
            let Some((key, value)) = line.split_once(':') else {
                continue;
            };
            let value = value.trim();
            match key {
                "id" => id = Some(value.to_string()),
                "level" => {
                    level = Some(value.parse::<u8>().unwrap_or_else(|_| {
                        panic!("{}: `level: {value}` is not a number", path.display())
                    }));
                }
                "withdrawn" => withdrawn = value == "true",
                "invariants" => {
                    let inner = value
                        .strip_prefix('[')
                        .and_then(|v| v.strip_suffix(']'))
                        .unwrap_or_else(|| {
                            panic!(
                                "{}: `invariants:` must be an inline list such as `[I5, I20]`, \
                                 found `{value}`",
                                path.display()
                            )
                        });
                    invariants = Some(
                        inner
                            .split(',')
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                            .map(str::to_string)
                            .collect::<Vec<String>>(),
                    );
                }
                _ => {}
            }
        }

        Case {
            id: id.unwrap_or_else(|| panic!("{}: no top-level `id:`", path.display())),
            level: level.unwrap_or_else(|| panic!("{}: no top-level `level:`", path.display())),
            invariants: invariants
                .unwrap_or_else(|| panic!("{}: no top-level `invariants:`", path.display())),
            withdrawn,
        }
    }

    /// The normative mapping, in machine-readable form (§18).
    fn cases_from_map() -> Vec<Case> {
        let path = repo_root().join("spec/conformance-map.json");
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
        let map: serde_json::Value = serde_json::from_str(&text)
            .unwrap_or_else(|e| panic!("{} is not JSON: {e}", path.display()));

        let mut cases: Vec<Case> = map["cases"]
            .as_array()
            .unwrap_or_else(|| panic!("{}: `cases` must be an array", path.display()))
            .iter()
            .map(|case| Case {
                id: case["id"].as_str().expect("case id").to_string(),
                level: case["level"].as_u64().expect("case level") as u8,
                invariants: case["invariants"]
                    .as_array()
                    .expect("case invariants")
                    .iter()
                    .map(|i| i.as_str().expect("invariant id").to_string())
                    .collect(),
                withdrawn: case
                    .get("withdrawn")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false),
            })
            .collect();
        cases.sort_by(|a, b| a.id.cmp(&b.id));
        cases
    }

    /// Which cases prove each invariant.
    fn coverage_of(cases: &[Case]) -> BTreeMap<u8, Vec<String>> {
        let mut coverage: BTreeMap<u8, Vec<String>> = BTreeMap::new();
        for case in cases.iter().filter(|c| !c.withdrawn) {
            for name in &case.invariants {
                let number: u8 = name
                    .strip_prefix('I')
                    .and_then(|n| n.parse().ok())
                    .unwrap_or_else(|| {
                        panic!(
                            "case {} names `{name}`, which is not an invariant id",
                            case.id
                        )
                    });
                assert!(
                    get(number).is_some(),
                    "case {} names `{name}`, which §17 does not define",
                    case.id
                );
                coverage.entry(number).or_default().push(case.id.clone());
            }
        }
        coverage
    }

    /// The gap report the meta-test asserts on: every invariant with no case proving it.
    ///
    /// Factored out so the negative test below can drive the *same* logic with coverage removed. A
    /// meta-test nobody has watched fail is a meta-test nobody knows works.
    fn uncovered(coverage: &BTreeMap<u8, Vec<String>>) -> Vec<String> {
        INVARIANTS
            .iter()
            .filter(|i| !coverage.contains_key(&i.id.number()))
            .map(|i| format!("{} — {}", i.id, i.text))
            .collect()
    }

    fn gap_report(uncovered: &[String]) -> String {
        format!(
            "these invariants have no conformance case in conformance/cases/, so nothing stops \
             them regressing:\n  {}",
            uncovered.join("\n  ")
        )
    }

    /// **The meta-test.** Every invariant I1..I21 must have at least one conformance case that
    /// actually exists on disk, is not withdrawn, and declares it.
    ///
    /// A case named in the specification but never written does not count here, which is the whole
    /// point: coverage is what the suite executes, not what a table claims about it.
    #[test]
    fn every_invariant_has_at_least_one_conformance_case() {
        let coverage = coverage_of(&cases_from_files());
        let uncovered = uncovered(&coverage);
        assert!(uncovered.is_empty(), "{}", gap_report(&uncovered));
        assert_eq!(coverage.len(), INVARIANTS.len());
    }

    /// The meta-test must actually fail when coverage disappears.
    ///
    /// Drives the real case data with one invariant's mapping stripped and asserts the gap is both
    /// detected and named. Without this, "every invariant is covered" and "the check is broken" look
    /// identical from the outside — which is the failure mode a meta-test exists to prevent, so it
    /// would be an odd one to leave in the meta-test itself.
    #[test]
    fn the_meta_test_fails_when_an_invariant_loses_its_only_case() {
        let mut cases = cases_from_files();
        assert!(
            uncovered(&coverage_of(&cases)).is_empty(),
            "precondition: all covered"
        );

        // I12 is proved by exactly one case today, so removing it from that case removes the
        // invariant's entire coverage.
        for case in &mut cases {
            case.invariants.retain(|i| i != "I12");
        }

        let gaps = uncovered(&coverage_of(&cases));
        assert_eq!(gaps.len(), 1, "exactly one invariant should be uncovered");
        assert!(gaps[0].starts_with("I12 — "), "{}", gaps[0]);
        let report = gap_report(&gaps);
        assert!(report.contains("nothing stops them regressing"));
        assert!(
            report.contains("Every state transition emits its event in the same transaction"),
            "the report must name the invariant in words, not just by number:\n{report}"
        );
    }

    /// The suite on disk and the normative mapping must agree, in both directions.
    ///
    /// A case in the map with no file is a case nobody runs. A file the map does not list is a case
    /// the specification does not know about. Either is drift, and neither is visible from the
    /// conformance runner's own output.
    #[test]
    fn the_case_files_and_the_normative_map_agree() {
        let files = cases_from_files();
        let map = cases_from_map();

        let file_ids: BTreeSet<&str> = files.iter().map(|c| c.id.as_str()).collect();
        let map_ids: BTreeSet<&str> = map.iter().map(|c| c.id.as_str()).collect();
        let missing_files: Vec<&&str> = map_ids.difference(&file_ids).collect();
        let unmapped_files: Vec<&&str> = file_ids.difference(&map_ids).collect();
        assert!(
            missing_files.is_empty(),
            "conformance-map.json lists cases with no file in conformance/cases/: {missing_files:?}"
        );
        assert!(
            unmapped_files.is_empty(),
            "conformance/cases/ holds cases the normative map does not list: {unmapped_files:?}"
        );

        // And they must agree on what each case proves, not merely on which cases exist.
        assert_eq!(
            coverage_of(&files),
            coverage_of(&map),
            "the case files and conformance-map.json disagree about which cases prove which \
             invariants"
        );
        for (file, mapped) in files.iter().zip(map.iter()) {
            assert_eq!(
                file.level, mapped.level,
                "case {} disagrees on level",
                file.id
            );
        }
    }

    /// The parser must reject a case it cannot fully read, rather than skipping it.
    #[test]
    fn an_unreadable_case_is_a_failure_not_a_skip() {
        let complete = "id: C-1\nlevel: 1\ninvariants: [I20]\n";
        assert_eq!(
            parse_case(complete, Path::new("test.yaml")).invariants,
            vec!["I20"]
        );

        for incomplete in [
            "level: 1\ninvariants: [I20]\n",             // no id
            "id: C-1\ninvariants: [I20]\n",              // no level
            "id: C-1\nlevel: 1\n",                       // no invariants
            "id: C-1\nlevel: 1\ninvariants: I20\n",      // not an inline list
            "  id: C-1\n  level: 1\n  invariants: []\n", // indented: belongs to a step
        ] {
            assert!(
                std::panic::catch_unwind(|| parse_case(incomplete, Path::new("test.yaml")))
                    .is_err(),
                "must refuse to parse:\n{incomplete}"
            );
        }
    }

    /// A withdrawn case proves nothing, so withdrawing the sole prover of an invariant must break
    /// coverage rather than quietly reducing it.
    ///
    /// This is also the standing proof that the meta-test can fail: it removes real coverage from
    /// real case data and asserts the gap appears.
    #[test]
    fn a_withdrawn_case_stops_counting_as_coverage() {
        let mut cases = cases_from_files();
        let before = coverage_of(&cases);
        // I12 is proved by exactly one case today, which makes it the sharpest probe available.
        let sole_prover = before.get(&12).expect("I12 is covered").clone();
        assert_eq!(
            sole_prover.len(),
            1,
            "I12 is expected to have exactly one prover"
        );

        for case in cases.iter_mut().filter(|c| c.id == sole_prover[0]) {
            case.withdrawn = true;
        }
        assert!(
            !coverage_of(&cases).contains_key(&12),
            "withdrawing {} must leave I12 uncovered",
            sole_prover[0]
        );
    }
}
