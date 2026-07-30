//! The Handoff conformance suite.
//!
//! Takes a base URL and runs the declarative cases in `conformance/cases/` against whatever is
//! listening there. It does not care whether that is the reference server, someone else's
//! implementation, or a hosted service — which is the whole point, and the reason this crate is the
//! project's governance instrument rather than a test helper.
//!
//! Three things follow from that, all of them policy rather than implementation detail:
//!
//! - **"Handoff-compatible" means a published passing run** for a stated version. Not a badge, not
//!   a claim. See `TRADEMARKS.md`.
//! - **A behaviour change in the core without a case here is not merged.** This is what stops the
//!   suite lagging the implementation, which is the normal way a conformance suite dies.
//! - **The suite gates deploys, including ours.** A hosted service that cannot pass the open suite
//!   turns the build red. That converts "we did not fork the core" from an intention into a check
//!   anyone can rerun.
//!
//! # Design constraints this crate holds itself to
//!
//! - **HTTP only.** No dependency on `handoff-protocol`, `handoff-core`, or any store crate, and no
//!   database driver. A shared type between the suite and the reference implementation would let
//!   one bug cancel out the other, and an implementation written in another language must be able
//!   to run this binary unmodified.
//! - **Cases are data.** Every case-specific fact lives in a YAML file a non-Rust implementer can
//!   read and argue with. This crate is an interpreter for that format and holds no knowledge of
//!   any individual case. The format is documented in `CASE-FORMAT.md`.
//! - **Nothing is ever skipped.** A case a deployment has not configured is a **failure** with a
//!   stated reason. A suite that silently skips what it cannot run reports conformance it did not
//!   measure, and that is worse than no suite at all.
//!
//! # Anything below the HTTP API
//!
//! Two requirements in the specification cannot be asserted over HTTP by construction. C-15 must
//! attempt a receipt mutation **at the storage layer**, because §9.4 puts the application inside
//! the threat model. C-7 must grep the deployment's logs. Both are expressed as *hooks*: the
//! deployment supplies a command in its profile, this suite invokes it, and the case asserts the
//! outcome. A missing hook fails the case.

#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![deny(rustdoc::broken_intra_doc_links)]

pub mod callback;
pub mod case;
pub mod expect;
pub mod http;
pub mod profile;
pub mod report;
pub mod runner;
pub mod signing;
pub mod vars;

use std::path::{Path, PathBuf};

/// Version of this crate, as published to crates.io.
pub const CRATE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Protocol version this suite is written against. A conformance claim names both.
pub const PROTOCOL_VERSION: &str = "0.1";

/// Specification §18, as published in machine-readable form.
///
/// §18 states that the case table "is duplicated in machine-readable form at
/// `conformance-map.json` so that an implementation can iterate it without parsing this table", so
/// the suite iterates it rather than carrying its own copy. A case list hardcoded here would drift
/// from the specification silently — which is the failure mode a conformance suite is least able to
/// afford, since nothing else in the project would notice.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct ConformanceMap {
    /// Protocol version the map describes.
    pub spec_version: String,
    /// Every case §18 defines.
    pub cases: Vec<MappedCase>,
    /// Every invariant §17 defines, with the cases that prove it.
    pub invariants: Vec<MappedInvariant>,
}

/// One case as §18 defines it.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct MappedCase {
    /// Stable case id.
    pub id: String,
    /// Conformance level.
    pub level: u8,
    /// §18's own wording of the test.
    pub title: String,
    /// The invariants this case proves.
    pub invariants: Vec<String>,
}

/// One invariant as §17 defines it.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct MappedInvariant {
    /// Stable invariant id.
    pub id: String,
    /// The invariant itself.
    pub statement: String,
    /// The cases that prove it.
    pub cases: Vec<String>,
}

impl ConformanceMap {
    /// Load the map from a repository root.
    pub fn load(repo_root: &Path) -> Result<Self, String> {
        let path = repo_root.join("spec").join("conformance-map.json");
        let text = std::fs::read_to_string(&path).map_err(|e| {
            format!(
                "cannot read {}: {e}. §18 publishes the case table in machine-readable form and \
                 this suite iterates it rather than keeping its own copy, so without it there is \
                 nothing to measure against.",
                path.display()
            )
        })?;
        serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// Case ids at or below a conformance level.
    pub fn ids_at_level(&self, level: u8) -> Vec<&str> {
        self.cases
            .iter()
            .filter(|c| c.level <= level)
            .map(|c| c.id.as_str())
            .collect()
    }
}

/// Load every case file in a directory, sorted by file name.
pub fn load_cases(dir: &Path) -> Result<Vec<case::Case>, String> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| format!("cannot read the case directory {}: {e}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "yaml" || x == "yml"))
        .collect();
    files.sort();

    let mut cases = Vec::with_capacity(files.len());
    for file in files {
        let text = std::fs::read_to_string(&file)
            .map_err(|e| format!("cannot read {}: {e}", file.display()))?;
        cases.push(case::parse(&file, &text)?);
    }
    Ok(cases)
}

/// Check the loaded cases against the published map, so drift from §18 cannot go unnoticed.
///
/// This runs before every suite invocation, not only in CI. A case file that has been deleted, a
/// case §18 added that nobody wrote, a level that disagrees, or an invariant claim the map does not
/// support all stop the run — because each of them would otherwise produce a total that looks like
/// a measurement and is not one.
pub fn audit_coverage(cases: &[case::Case], map: &ConformanceMap) -> Result<(), String> {
    let mut problems = Vec::new();
    let ids: Vec<&str> = cases.iter().map(|c| c.id.as_str()).collect();

    for expected in &map.cases {
        if !ids.contains(&expected.id.as_str()) {
            problems.push(format!(
                "§18 defines {} ({}) and no case file implements it",
                expected.id, expected.title
            ));
        }
    }
    for (i, id) in ids.iter().enumerate() {
        if ids[..i].contains(id) {
            problems.push(format!("{id} is defined by more than one case file"));
        }
        if !map.cases.iter().any(|c| c.id == *id) {
            problems.push(format!("{id} is not a case §18 defines"));
        }
    }
    for case in cases {
        let Some(mapped) = map.cases.iter().find(|c| c.id == case.id) else {
            continue;
        };
        if case.level != mapped.level {
            problems.push(format!(
                "{} declares level {} but §18 places it at Level {}",
                case.id, case.level, mapped.level
            ));
        }
        let mut declared = case.invariants.clone();
        let mut required = mapped.invariants.clone();
        declared.sort();
        required.sort();
        if declared != required {
            problems.push(format!(
                "{} claims invariants {declared:?} but §18 maps it to {required:?}",
                case.id
            ));
        }
    }
    for invariant in &map.invariants {
        if !invariant.cases.iter().any(|c| ids.contains(&c.as_str())) {
            problems.push(format!(
                "{} ({}) has no implemented case",
                invariant.id, invariant.statement
            ));
        }
    }

    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems.join("\n"))
    }
}

/// Find the repository root by walking up from a starting directory looking for `conformance/cases`.
pub fn find_repo_root(start: &Path) -> Option<PathBuf> {
    let mut dir = Some(start.to_path_buf());
    while let Some(current) = dir {
        if current.join("conformance").join("cases").is_dir() {
            return Some(current);
        }
        dir = current.parent().map(Path::to_path_buf);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crate_version_is_populated() {
        assert!(!CRATE_VERSION.is_empty());
    }

    #[test]
    fn every_case_file_in_the_repository_loads_and_covers_section_18() {
        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let Some(root) = find_repo_root(&manifest) else {
            return;
        };
        let map = ConformanceMap::load(&root).expect("§18 map loads");
        let cases = load_cases(&root.join("conformance").join("cases")).expect("cases load");
        audit_coverage(&cases, &map).expect("cases cover §18");
        assert_eq!(map.spec_version, PROTOCOL_VERSION);
    }
}
