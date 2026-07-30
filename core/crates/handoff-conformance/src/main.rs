//! `handoff-conformance` — run the Handoff conformance suite against any base URL.

use handoff_conformance::{
    audit_coverage, find_repo_root, load_cases, profile::Profile, report, runner::Runner,
    ConformanceMap, CRATE_VERSION, PROTOCOL_VERSION,
};
use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "\
handoff-conformance — the executable half of the Handoff specification

USAGE:
    handoff-conformance --base-url <URL> [OPTIONS]

OPTIONS:
    --base-url <URL>     Base URL of the deployment under test, including the /v1 prefix.
    --profile <FILE>     Deployment profile: credentials, deployment choices, and the hooks the
                         cases need below the HTTP API. Without one, every case fails with a
                         stated reason — which is the correct result for a deployment that has
                         demonstrated nothing.
    --cases <DIR>        Case directory. Defaults to conformance/cases, found by walking up.
    --level <1|2>        1 (default) runs the cases every Server must pass. 2 adds C-17, the
                         optional continuation extension of §14.
    --case <ID>          Run one case: --case C-3, or --case c03.
    --list               Print the case list with the invariants each covers, and exit.
    -V, --version        Print versions and exit.
    -h, --help           Print this help and exit.

EXIT CODE:
    0 only when every Level 1 case passed. Anything else, including a case that could not be run,
    is a non-zero exit.
";

fn main() -> ExitCode {
    match run() {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(message) => {
            eprintln!("handoff-conformance: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<bool, String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |name: &str| args.iter().any(|a| a == name);
    let value = |name: &str| -> Option<String> {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };

    if flag("-h") || flag("--help") {
        print!("{USAGE}");
        return Ok(true);
    }
    if flag("-V") || flag("--version") {
        println!("handoff-conformance {CRATE_VERSION} (protocol {PROTOCOL_VERSION})");
        return Ok(true);
    }

    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let cwd = std::env::current_dir().unwrap_or_else(|_| manifest.clone());
    let repo_root = find_repo_root(&cwd)
        .or_else(|| find_repo_root(&manifest))
        .ok_or("cannot find conformance/cases; pass --cases <DIR>")?;

    let cases_dir = value("--cases")
        .map(PathBuf::from)
        .unwrap_or_else(|| repo_root.join("conformance").join("cases"));

    let map = ConformanceMap::load(&repo_root)?;
    let cases = load_cases(&cases_dir)?;
    audit_coverage(&cases, &map).map_err(|problems| {
        format!("the case set does not match specification §18:\n{problems}")
    })?;

    if flag("--list") {
        for case in &cases {
            println!(
                "{:<6} L{}  {:<62} [{}]",
                case.id,
                case.level,
                case.title,
                case.invariants.join(" ")
            );
        }
        println!(
            "\n{} cases loaded from {}",
            cases.len(),
            cases_dir.display()
        );
        println!(
            "checked against spec/conformance-map.json (protocol {})",
            map.spec_version
        );
        for invariant in &map.invariants {
            println!(
                "{:<5} proved by {}",
                invariant.id,
                invariant.cases.join(", ")
            );
        }
        return Ok(true);
    }

    let level: u8 = match value("--level") {
        Some(v) => v
            .parse()
            .map_err(|_| format!("--level must be 1 or 2, not {v:?}"))?,
        None => 1,
    };
    if level != 1 && level != 2 {
        return Err(format!("--level must be 1 or 2, not {level}"));
    }

    let profile = match value("--profile") {
        Some(path) => Profile::load(&PathBuf::from(path))?,
        None => Profile::default(),
    };

    let base_url = value("--base-url")
        .or_else(|| profile.base_url.clone())
        .ok_or("no --base-url. The suite tests a deployment; it needs one to talk to.")?;

    let selected: Vec<_> = match value("--case") {
        Some(wanted) => {
            let matches: Vec<_> = cases
                .iter()
                .filter(|c| same_case(&c.id, &wanted))
                .cloned()
                .collect();
            if matches.is_empty() {
                return Err(format!(
                    "no case {wanted:?}. Run --list to see the {} cases this suite defines.",
                    cases.len()
                ));
            }
            matches
        }
        None => cases.iter().filter(|c| c.level <= level).cloned().collect(),
    };

    println!("handoff-conformance {CRATE_VERSION} — protocol {PROTOCOL_VERSION}, Level {level}");
    println!("target: {base_url}");
    println!("cases:  {} from {}", selected.len(), cases_dir.display());
    if value("--profile").is_none() {
        println!(
            "note:   no --profile, so no credentials and no hooks. Every case will fail with the \
             reason stated."
        );
    }
    println!();

    let runner = Runner::new(&base_url, &profile, repo_root);
    let results: Vec<_> = selected.iter().map(|c| runner.run(c)).collect();
    Ok(report::print(&results, level))
}

/// Accept `C-3`, `c3`, `c03`, and `C-6b` for the same case.
fn same_case(id: &str, wanted: &str) -> bool {
    fn normalize(text: &str) -> String {
        let lowered = text.to_lowercase().replace('-', "");
        let digits: String = lowered.chars().filter(|c| c.is_ascii_digit()).collect();
        let suffix: String = lowered
            .chars()
            .skip_while(|c| !c.is_ascii_digit())
            .filter(|c| c.is_ascii_alphabetic())
            .collect();
        format!("c{}{suffix}", digits.trim_start_matches('0'))
    }
    normalize(id) == normalize(wanted)
}

#[cfg(test)]
mod tests {
    use super::same_case;

    #[test]
    fn case_selection_accepts_the_forms_people_type() {
        assert!(same_case("C-3", "c03"));
        assert!(same_case("C-3", "C-3"));
        assert!(same_case("C-6b", "c06b"));
        assert!(!same_case("C-6", "C-6b"));
        assert!(!same_case("C-1", "C-11"));
    }
}
