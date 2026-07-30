//! `handoff-conformance` — run the Handoff conformance suite against any base URL.

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();

    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("handoff-conformance {}", handoff_conformance::CRATE_VERSION);
        return ExitCode::SUCCESS;
    }

    let base_url = args
        .iter()
        .position(|a| a == "--base-url")
        .and_then(|i| args.get(i + 1))
        .cloned();

    match base_url {
        Some(url) => eprintln!("target: {url}"),
        None => eprintln!("usage: handoff-conformance --base-url <url>"),
    }

    eprintln!("{}/{} cases passing", 0, handoff_conformance::CASE_COUNT);
    eprintln!();
    eprintln!("TODO(H1): the conformance suite does not exist yet. This runner reports zero cases");
    eprintln!("and exits non-zero on purpose, so that no pipeline can report conformance it has");
    eprintln!("not measured. It goes green in H2, against the reference server.");
    eprintln!();
    eprintln!(
        "Contributing a case: https://github.com/OmegaAgent/handoff/blob/main/CONTRIBUTING.md"
    );
    ExitCode::FAILURE
}
