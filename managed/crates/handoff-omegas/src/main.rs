//! `handoff-omegas-server` — the managed deployment's binary.
//!
//! Deliberately thin, and its thinness is the deliverable. It reads configuration, builds the
//! adapters, hands them to the **open** `handoff-server`, and starts two background loops. It
//! contains no route, no state transition, and no protocol decision, because every one of those
//! would be a behaviour the open core does not have.
//!
//! If this file ever grows a handler, the boundary has moved and somebody should notice in review.

use handoff_omegas::{auth, config, control_plane, reconciler, Managed};
use handoff_protocol::error::{ErrorCode, ProtocolError, Result};
use handoff_server::state::Deployment;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

const USAGE: &str = "\
handoff-omegas-server — the managed Handoff deployment

It is the open `handoff-server` plus one adapter crate. Everything the protocol defines lives in
the open core; everything here is an implementation of one of its ports.

USAGE:
    handoff-omegas-server [serve]     Serve the /v1 API (the default).
    handoff-omegas-server preflight   Report every control-plane dependency and exit.
    handoff-omegas-server --version

ENVIRONMENT (the open server's own variables apply unchanged; these are additional):
    OMEGAS_CONTROL_PLANE_BASE      Base URL of the Ωmegas control plane.
    OMEGAS_SERVICE_TOKEN           This service's own credential. Never a customer's key.
    OMEGAS_TOKEN_ISSUER            Issuer whose short-lived tokens are accepted. Blocked on M5.
    OMEGAS_JWKS_URL                 Where that issuer publishes its public keys.
    OMEGAS_AUDIENCE                 What this service is called in a token.
    OMEGAS_CONTACT_POINTS_AVAILABLE Whether per-person contact records exist yet. Default false.
    OMEGAS_RECONCILE_INTERVAL_MS    Reconciler and outbox drain interval. Default 5000.
    OMEGAS_ALLOW_NO_AUTH            Start a deployment that authenticates nobody, on purpose.
";

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "handoff_omegas=info,handoff_server=info".into()),
        )
        .init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("{}", handoff_omegas::version_line());
        return ExitCode::SUCCESS;
    }
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print!("{USAGE}");
        return ExitCode::SUCCESS;
    }

    match run(args.first().map(String::as_str).unwrap_or("serve")).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("handoff-omegas-server: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Report what is absent, on every boot and as its own command.
///
/// A deployment that quietly runs without metering, without entitlements, and without attestation
/// looks identical to one that has them. Printing the list is the cheapest possible way to keep the
/// gap visible to the person deploying it.
fn report(settings: &config::OmegasConfig) {
    for missing in settings.preflight() {
        tracing::warn!("{missing}");
    }
}

async fn run(command: &str) -> Result<()> {
    let settings = config::OmegasConfig::from_env()?;

    if command == "preflight" {
        for missing in settings.preflight() {
            println!("{missing}");
        }
        return Ok(());
    }
    if command != "serve" {
        return Err(ProtocolError::new(
            ErrorCode::InvalidRequest,
            format!("unknown command {command:?}. Run with --help."),
        ));
    }

    report(&settings);
    if std::env::var("OMEGAS_ALLOW_NO_AUTH").ok().as_deref() != Some("1") {
        settings.require_authentication()?;
    }

    // The open server's own configuration, unchanged. The managed deployment does not get a
    // different one: a second configuration surface is a second set of defaults to drift apart.
    let open = handoff_server::config::Config::from_env()?;

    let transport: Box<dyn control_plane::Transport> = Box::new(control_plane::HttpTransport::new(
        &settings.control_plane_base,
        &settings.service_token,
    )?);
    let pool = sqlx::PgPool::connect(&open.database_url)
        .await
        .map_err(|e| {
            ProtocolError::new(
                ErrorCode::InvalidRequest,
                format!("the managed adapter cannot reach its own database: {e}"),
            )
        })?;
    let managed = Managed::assemble(&settings, transport, pool).await?;

    let authenticator = auth::OmegasAuthenticator::new(
        Box::new(auth::HttpJwks::new(&settings.jwks_url)?),
        auth::TokenPolicy {
            issuer: settings.token_issuer.clone(),
            audience: settings.audience.clone(),
        },
    );

    // The one hook: where a credential is verified. Everything else this crate does is driven from
    // the loops below, outside any request, so the open server does not know the rest exists.
    let state = handoff_server::build_with(
        open,
        Deployment {
            authenticator: Some(Arc::new(authenticator)),
        },
    )
    .await?;

    let reconciler = Arc::new(reconciler::Reconciler::new(
        Box::new(reconciler::StoreReceipts::new(Arc::clone(&state.store))),
        Arc::clone(&managed.outbox),
    ));

    tokio::spawn(drain_loop(
        Arc::clone(&managed.outbox),
        Arc::clone(&managed.events),
        Arc::clone(&managed.meter),
        Duration::from_millis(settings.reconcile_interval_ms),
    ));
    // The reconciler is spawned holding a reference it does not yet use: tenant discovery has no
    // home (see `Reconciler::run_for`). Keeping the wiring here rather than deleting it makes the
    // gap a visible one-line TODO in a running binary instead of an unwritten paragraph.
    drop(reconciler);

    // `serve` starts the open server's own delivery loop over the adapters it shipped with. The
    // managed fleet — the reviewed Slack app, the warmed SES identity, the numbers — is not wired
    // here because none of those transports exist in this tree and none of their operational assets
    // can be checked in. `delivery::fleet_capabilities` declares what the fleet is meant to be;
    // registering real transports is the follow-up, and it is an argument to the open server's
    // delivery loop rather than a change to it.
    tracing::info!("{}", handoff_omegas::version_line());
    handoff_server::serve(state).await
}

/// Drain the outbox forever, and report depth.
///
/// A queue nobody reads is a queue that loses data quietly — which is exactly defect B-25 in the
/// control plane's own outbox. The depth and the stuck count are logged every pass so that the
/// deployment has something to alarm on from day one.
async fn drain_loop(
    outbox: Arc<handoff_omegas::outbox::Outbox>,
    events: Arc<handoff_omegas::events::OmegasEvents>,
    meter: Arc<handoff_omegas::meter::OmegasMeter>,
    interval: Duration,
) {
    loop {
        match outbox.drain(events.as_ref(), meter.as_ref(), 100).await {
            Ok(acked) if acked > 0 => tracing::info!(acked, "mirrored to the control plane"),
            Ok(_) => {}
            Err(error) => tracing::error!("the outbox drain failed: {error}"),
        }
        match outbox.stuck().await {
            Ok(stuck) if !stuck.is_empty() => {
                tracing::error!(
                    count = stuck.len(),
                    first = %stuck[0].1,
                    detail = %stuck[0].2,
                    "audit mirror rows are stuck and are NOT being dropped -- somebody must look"
                );
            }
            Ok(_) => {}
            Err(error) => tracing::error!("cannot read stuck outbox rows: {error}"),
        }
        tokio::time::sleep(interval).await;
    }
}
