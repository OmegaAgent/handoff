//! `handoffd` — the Handoff reference server.

use handoff_server::{cli, config::Config};
use std::process::ExitCode;

const USAGE: &str = "\
handoffd — the Handoff reference server

USAGE:
    handoffd [serve]                      Serve the /v1 API (the default).
    handoffd migrate                      Apply migrations and exit.
    handoffd verify-chain [--tenant REF]  Re-walk every receipt chain. Exit 0 only if all verify.
    handoffd dump-events --request ID     Print this request's event types, one per line.
    handoffd inject-channel-message --request ID --channel NAME --text TEXT
    handoffd observe-page-change --request ID
    handoffd canonicalize --path FILE     RFC 8785 bytes, then bytes=<n> and sha256=<hex>.
    handoffd canonicalize --json TEXT     The canonical bytes alone, as a filter. No database.
    handoffd --version

ENVIRONMENT:
    HANDOFF_DATABASE_URL              Required. handoffd owns this database.
    HANDOFF_BIND                      Default 127.0.0.1:8080.
    HANDOFF_PUBLIC_BASE               Base for surface_url. Default http://127.0.0.1:8080.
    HANDOFF_LINK_ONLY_PERMITTED       Whether link_only may settle a request. Default false.
    HANDOFF_CALLBACK_SECRETS          Comma-separated active signing secrets. Two during a rotation.
    HANDOFF_CAPABILITY_TRANSPORT_BASE Scheme and authority for resolved surfaces.
    HANDOFF_BOOTSTRAP                 A JSON file of credentials to seed.
    HANDOFF_SWEEP_INTERVAL_MS         How often deadlines are swept. Default 500.
";

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "handoff_server=info,handoff_store_postgres=info".into()),
        )
        .init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("{}", handoff_server::version_line());
        return ExitCode::SUCCESS;
    }
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print!("{USAGE}");
        return ExitCode::SUCCESS;
    }

    match run(&args).await {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(error) => {
            eprintln!("handoffd: {error}");
            ExitCode::FAILURE
        }
    }
}

fn value(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

async fn run(args: &[String]) -> handoff_protocol::error::Result<bool> {
    // Canonicalization is a pure function over bytes. It needs no store, and requiring one would
    // stop an auditor with a receipt and no database from checking it.
    if args.first().map(String::as_str) == Some("canonicalize") {
        let path = value(args, "--path");
        let document = cli::read_document(path.as_deref(), value(args, "--json").as_deref())?;
        cli::canonicalize(&document, path.is_some())?;
        return Ok(true);
    }

    let config = Config::from_env()?;
    let command = args.first().map(String::as_str).unwrap_or("serve");

    match command {
        "serve" => {
            let state = handoff_server::build(config).await?;
            handoff_server::serve(state).await?;
            Ok(true)
        }
        "migrate" => {
            let state = handoff_server::build(config).await?;
            println!("migrations applied");
            drop(state);
            Ok(true)
        }
        "verify-chain" => {
            let state = handoff_server::build(config).await?;
            cli::verify_chains(state.store.as_ref(), value(args, "--tenant").as_deref()).await
        }
        "dump-events" => {
            let request = value(args, "--request").ok_or_else(missing_request)?;
            let state = handoff_server::build(config).await?;
            cli::dump_events(state.store.as_ref(), &request).await?;
            Ok(true)
        }
        "inject-channel-message" => {
            let request = value(args, "--request").ok_or_else(missing_request)?;
            let channel = value(args, "--channel").unwrap_or_else(|| "email".into());
            let text = value(args, "--text").unwrap_or_default();
            let state = handoff_server::build(config).await?;
            cli::inject_channel_message(state.store.as_ref(), &request, &channel, &text).await?;
            Ok(true)
        }
        "observe-page-change" => {
            let request = value(args, "--request").ok_or_else(missing_request)?;
            let state = handoff_server::build(config).await?;
            cli::observe_page_change(state.store.as_ref(), &request).await?;
            Ok(true)
        }
        other => Err(handoff_protocol::error::ProtocolError::new(
            handoff_protocol::error::ErrorCode::InvalidRequest,
            format!("unknown command {other:?}. Run `handoffd --help`."),
        )),
    }
}

fn missing_request() -> handoff_protocol::error::ProtocolError {
    handoff_protocol::error::ProtocolError::new(
        handoff_protocol::error::ErrorCode::InvalidRequest,
        "--request is required",
    )
}
