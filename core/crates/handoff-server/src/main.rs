//! `handoffd` — the Handoff reference server.

use std::process::ExitCode;

fn main() -> ExitCode {
    if std::env::args().any(|a| a == "--version" || a == "-V") {
        println!("{}", handoff_server::version_line());
        return ExitCode::SUCCESS;
    }

    eprintln!("{}", handoff_server::version_line());
    eprintln!();
    eprintln!("This build serves no protocol surface. handoffd is a skeleton: the state machine");
    eprintln!("lands in milestone H1 and the server in H2. Nothing is listening, and nothing here");
    eprintln!("should be pointed at a client.");
    eprintln!();
    eprintln!("Track it: https://github.com/OmegaAgent/handoff");
    ExitCode::FAILURE
}
