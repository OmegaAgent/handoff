//! A disposable deployment: its own database, its own `handoffd`, torn down at the end.
//!
//! These tests use a real Postgres and a real process because the two properties they exist to
//! check — that a durable wait survives `kill -9`, and that row-level security holds per table —
//! are properties of the database and the process, and a mock of either would be a mock of the
//! thing under test.

use std::io::Write;
use std::process::{Child, Command, Stdio};

/// Where the tests reach Postgres. The database they create is disposable and named per run.
pub fn admin_url() -> String {
    std::env::var("HANDOFF_TEST_ADMIN_URL")
        .unwrap_or_else(|_| "postgres://omega:omega@localhost:5432/postgres".to_string())
}

fn database_url(name: &str) -> String {
    let admin = admin_url();
    let (base, _) = admin
        .rsplit_once('/')
        .expect("an admin URL names a database");
    format!("{base}/{name}")
}

/// The same host and port as [`admin_url`], as some other role.
fn role_url(name: &str, role: &str) -> String {
    let admin = admin_url();
    let (scheme, rest) = admin.split_once("://").expect("an admin URL has a scheme");
    let hosted = rest.rsplit_once('@').map(|(_, h)| h).unwrap_or(rest);
    let (host, _) = hosted
        .rsplit_once('/')
        .expect("an admin URL names a database");
    format!("{scheme}://{role}:{LEAST_PRIVILEGE_PASSWORD}@{host}/{name}")
}

/// The password every disposable role in this suite is created with. Not a secret: these roles
/// exist for the length of one test against a local development cluster.
pub const LEAST_PRIVILEGE_PASSWORD: &str = "least-privilege";

/// A running deployment.
pub struct Deployment {
    /// Name of the disposable database.
    pub database: String,
    /// Connection URL for it.
    pub url: String,
    /// Where the API is listening.
    pub base: String,
    /// The bootstrap file this deployment seeded from.
    pub bootstrap: std::path::PathBuf,
    port: u16,
    server: Option<Child>,
    env: Vec<(String, String)>,
    /// The role `handoffd` connects as, when it is not the development superuser. Roles are
    /// cluster-wide, so dropping the database does not take one with it.
    role: Option<String>,
}

/// The machine credential for tenant A.
pub const MACHINE_A: &str = "test_machine_a";
/// The machine credential for tenant B.
pub const MACHINE_B: &str = "test_machine_b";
/// A person who may decide, in tenant A.
pub const EDITOR_A: &str = "test_editor_a";
/// A person who may decide, in tenant B.
pub const EDITOR_B: &str = "test_editor_b";
/// Tenant A.
pub const ORG_A: &str = "org_01K3M7QW8ZC4YRXB2N6VD9FTHA";
/// Tenant B.
pub const ORG_B: &str = "org_01K3M7QW8ZC4YRXB2N6VD9FTHB";

impl Deployment {
    /// A port the OS says is free. Never a preferred number.
    ///
    /// Tests in one binary run in parallel, so a fixed port is a collision waiting for the next
    /// test file, and the failure is nasty: the loser fails to bind while the winner answers, so a
    /// test can silently exercise another test's server, or watch its own die mid-run. An earlier
    /// version tried a caller's preferred port first and fell back — which still handed the same
    /// number to two deployments whenever both asked at once. Preferring nothing removes the class.
    fn free_port() -> u16 {
        std::net::TcpListener::bind("127.0.0.1:0")
            .and_then(|listener| listener.local_addr())
            .map(|address| address.port())
            .expect("the OS can assign a free port")
    }

    /// Create a database, seed credentials, and start `handoffd`.
    pub async fn start(label: &str) -> Deployment {
        Self::start_with(label, &[]).await
    }

    /// The same, with extra environment for the server process.
    ///
    /// Callback signing is the case this exists for: `HANDOFF_CALLBACK_SECRETS` is deployment
    /// configuration, and a test about rotation needs two of them, which no default can supply.
    pub async fn start_with(label: &str, env: &[(&str, &str)]) -> Deployment {
        Self::start_inner(label, env, false).await
    }

    /// A deployment whose `handoffd` connects as a role that **cannot bypass row-level security**.
    ///
    /// [`start`](Self::start) runs the server as the development superuser, which ignores every
    /// policy — so a test about what the policies do to `handoffd`'s own queries would pass against
    /// a database that had none. This one hands the database to a fresh login role and points
    /// `handoffd` at it.
    ///
    /// The role **owns** its tables rather than merely holding `SELECT, INSERT, UPDATE, DELETE` on
    /// them, and that is not a convenience: `handoffd` applies its migrations on every start, so a
    /// role that cannot issue DDL cannot start the process at all. Ownership does not weaken the
    /// test, because the policies are installed with `force row level security`, which keeps the
    /// owner subject to them — and `bypasses_row_level_security` asserts exactly that.
    pub async fn start_as_least_privilege(label: &str) -> Deployment {
        Self::start_inner(label, &[], true).await
    }

    async fn start_inner(label: &str, env: &[(&str, &str)], least_privilege: bool) -> Deployment {
        let database = format!(
            "handoff_test_{label}_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or_default()
        );

        psql(
            &admin_url(),
            &format!("drop database if exists \"{database}\" with (force)"),
        );
        let (url, role) = if least_privilege {
            // Unique per run, so two tests in parallel do not fight over one role.
            let role = format!("{database}_role");
            psql(&admin_url(), &format!("drop role if exists \"{role}\""));
            psql(
                &admin_url(),
                &format!("create role \"{role}\" login password '{LEAST_PRIVILEGE_PASSWORD}'"),
            );
            psql(
                &admin_url(),
                &format!("create database \"{database}\" owner \"{role}\""),
            );
            (role_url(&database, &role), Some(role))
        } else {
            psql(&admin_url(), &format!("create database \"{database}\""));
            (database_url(&database), None)
        };

        let bootstrap = std::env::temp_dir().join(format!("{database}.json"));
        let mut file = std::fs::File::create(&bootstrap).expect("a bootstrap file");
        write!(
            file,
            r#"{{"principals":[
                {{"id":"sa_01K3M7QW8ZC4YRXB2N6VD9FTH1","tenant_ref":"{ORG_A}","kind":"machine",
                  "token":"{MACHINE_A}","role":"admin","auth_strength":"session","scopes":["*"]}},
                {{"id":"usr_01K3M7QW8ZC4YRXB2N6VD9FTH3","tenant_ref":"{ORG_A}","kind":"human",
                  "token":"{EDITOR_A}","role":"editor","auth_strength":"session","scopes":["*"]}},
                {{"id":"sa_01K3M7QW8ZC4YRXB2N6VD9FTH2","tenant_ref":"{ORG_B}","kind":"machine",
                  "token":"{MACHINE_B}","role":"admin","auth_strength":"session","scopes":["*"]}},
                {{"id":"usr_01K3M7QW8ZC4YRXB2N6VD9FTH4","tenant_ref":"{ORG_B}","kind":"human",
                  "token":"{EDITOR_B}","role":"editor","auth_strength":"session","scopes":["*"]}}
            ]}}"#
        )
        .expect("write the bootstrap file");

        let port = Self::free_port();
        let mut deployment = Deployment {
            database,
            url,
            base: format!("http://127.0.0.1:{port}/v1"),
            bootstrap,
            port,
            server: None,
            env: env
                .iter()
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .collect(),
            role,
        };
        deployment.spawn().await;
        deployment
    }

    /// Start the server process and wait for it to answer.
    pub async fn spawn(&mut self) {
        let child = Command::new(env!("CARGO_BIN_EXE_handoffd"))
            .arg("serve")
            .envs(
                self.env
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone())),
            )
            .env("HANDOFF_DATABASE_URL", &self.url)
            .env("HANDOFF_BOOTSTRAP", &self.bootstrap)
            .env("HANDOFF_BIND", format!("127.0.0.1:{}", self.port))
            .env("HANDOFF_SWEEP_INTERVAL_MS", "250")
            // Several deployments run at once in this binary, and Postgres budgets connections
            // globally. Four each is ample for a test and leaves room for everything else.
            .env("HANDOFF_MAX_CONNECTIONS", "4")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("handoffd starts");
        self.server = Some(child);

        // `free_port` probes by binding and then *releases* the socket before `handoffd` binds it,
        // so two tests starting at the same moment can be handed the same port and one of them
        // loses the race. The loser's process exits immediately, and waiting twenty seconds for a
        // corpse to answer produced exactly the intermittent "connection refused" that made this
        // suite untrustworthy under load.
        //
        // So: notice that the child is gone, take a genuinely fresh port, and try again. The probe
        // cannot be made atomic without handing the listener to the child, and a bounded retry
        // buys the same reliability for a fraction of the complexity.
        for attempt in 0..8 {
            for _ in 0..100 {
                if reqwest::get(format!("{}/meta", self.base)).await.is_ok() {
                    return;
                }
                if let Some(server) = self.server.as_mut() {
                    if matches!(server.try_wait(), Ok(Some(_))) {
                        break; // it exited; almost always a lost race for the port
                    }
                }
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
            if attempt == 7 {
                break;
            }
            if let Some(mut server) = self.server.take() {
                let _ = server.kill();
                let _ = server.wait();
            }
            self.port = Self::free_port();
            self.base = format!("http://127.0.0.1:{}/v1", self.port);
            let child = Command::new(env!("CARGO_BIN_EXE_handoffd"))
                .arg("serve")
                .envs(
                    self.env
                        .iter()
                        .map(|(key, value)| (key.clone(), value.clone())),
                )
                .env("HANDOFF_DATABASE_URL", &self.url)
                .env("HANDOFF_BOOTSTRAP", &self.bootstrap)
                .env("HANDOFF_BIND", format!("127.0.0.1:{}", self.port))
                .env("HANDOFF_SWEEP_INTERVAL_MS", "250")
                .env("HANDOFF_MAX_CONNECTIONS", "4")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("handoffd starts");
            self.server = Some(child);
        }
        panic!("handoffd never answered, last port {}", self.port);
    }

    /// The process id, so a test can send it a signal.
    pub fn pid(&self) -> u32 {
        self.server.as_ref().expect("a running server").id()
    }

    /// `kill -9`. Not a shutdown: no handler runs, nothing is flushed, and anything the process was
    /// holding in memory is simply gone.
    pub fn kill_nine(&mut self) {
        let pid = self.pid();
        Command::new("kill")
            .args(["-9", &pid.to_string()])
            .status()
            .expect("kill -9");
        if let Some(mut child) = self.server.take() {
            let _ = child.wait();
        }
    }

    /// A connection to the deployment's database, for assertions below the API.
    ///
    /// As the development superuser, which is what an assertion wants: it reads the rows that
    /// exist, rather than the rows some policy is willing to show.
    pub async fn pool(&self) -> sqlx::PgPool {
        sqlx::postgres::PgPoolOptions::new()
            .max_connections(4)
            .connect(&database_url(&self.database))
            .await
            .expect("connect to the test database")
    }

    /// Run one statement against this deployment's database as the superuser, returning stdout.
    ///
    /// Panics rather than warning when `psql` fails. Setting up a condition and not noticing that
    /// the setup did not happen is how a test comes to measure nothing.
    pub fn superuser_sql(&self, sql: &str) -> String {
        let output = Command::new("psql")
            .args([&database_url(&self.database), "-q", "-tA", "-c", sql])
            .output()
            .expect("psql is available");
        assert!(
            output.status.success(),
            "psql: {sql}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    /// Run a `handoffd` maintenance subcommand against this deployment's database.
    ///
    /// Two of these subcommands are the only writer their table has: the protocol deliberately
    /// defines no inbound channel-adapter surface (§4.7) and no endpoint for recording a runtime
    /// observation (§9.7), so `inject-channel-message` and `observe-page-change` are how those rows
    /// come to exist at all. Returning stdout rather than a status keeps a caller able to assert on
    /// what the command *said*.
    pub fn run_handoffd(&self, args: &[&str]) -> String {
        let output = Command::new(env!("CARGO_BIN_EXE_handoffd"))
            .args(args)
            .env("HANDOFF_DATABASE_URL", &self.url)
            .env("HANDOFF_MAX_CONNECTIONS", "2")
            .output()
            .expect("handoffd runs");
        assert!(
            output.status.success(),
            "handoffd {}\n{}{}",
            args.join(" "),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    /// Whether the role `handoffd` connects as ignores every row-level-security policy.
    ///
    /// A test that tightens a policy and then watches the server's behaviour proves nothing unless
    /// this is false, because a superuser never sees a policy at all.
    pub fn bypasses_row_level_security(&self) -> bool {
        self.superuser_sql(&format!(
            "select bool_or(rolsuper or rolbypassrls) from pg_roles where rolname = {}",
            match &self.role {
                Some(role) => format!("'{role}'"),
                None => "current_user".to_string(),
            }
        )) == "t"
    }
}

impl Drop for Deployment {
    fn drop(&mut self) {
        if let Some(mut child) = self.server.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = std::fs::remove_file(&self.bootstrap);
        psql(
            &admin_url(),
            &format!("drop database if exists \"{}\" with (force)", self.database),
        );
        // Roles are cluster-wide, so the database going away does not take one with it — and a
        // cleanup that only ran on the happy path would leave one behind on every red run, which
        // is when tests are run most.
        if let Some(role) = &self.role {
            psql(&admin_url(), &format!("drop role if exists \"{role}\""));
        }
    }
}

fn psql(url: &str, sql: &str) {
    let output = Command::new("psql")
        .args([url, "-q", "-c", sql])
        .output()
        .expect("psql is available");
    if !output.status.success() {
        eprintln!("psql: {sql}\n{}", String::from_utf8_lossy(&output.stderr));
    }
}

/// POST a JSON body as a principal.
pub async fn post(
    base: &str,
    path: &str,
    token: &str,
    key: &str,
    body: serde_json::Value,
) -> (u16, serde_json::Value) {
    let response = reqwest::Client::new()
        .post(format!("{base}{path}"))
        .header("Authorization", format!("Bearer {token}"))
        .header("Idempotency-Key", key)
        .json(&body)
        .send()
        .await
        .expect("the server answers");
    let status = response.status().as_u16();
    let json = response.json().await.unwrap_or(serde_json::Value::Null);
    (status, json)
}

/// POST a JSON body as a principal, with no `Idempotency-Key`.
///
/// §3.1 makes the key caller-supplied, so "no key" is a real shape of the API rather than a test
/// shortcut — and it is a different code path, because a call without one stores no replay record.
pub async fn post_without_key(
    base: &str,
    path: &str,
    token: &str,
    body: serde_json::Value,
) -> (u16, serde_json::Value) {
    let response = reqwest::Client::new()
        .post(format!("{base}{path}"))
        .header("Authorization", format!("Bearer {token}"))
        .json(&body)
        .send()
        .await
        .expect("the server answers");
    let status = response.status().as_u16();
    let json = response.json().await.unwrap_or(serde_json::Value::Null);
    (status, json)
}

/// GET as a principal.
pub async fn get(base: &str, path: &str, token: &str) -> (u16, serde_json::Value) {
    let response = reqwest::Client::new()
        .get(format!("{base}{path}"))
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .await
        .expect("the server answers");
    let status = response.status().as_u16();
    let json = response.json().await.unwrap_or(serde_json::Value::Null);
    (status, json)
}

/// A raise body with one choice field.
pub fn raise_body(waiter: &str, title: &str) -> serde_json::Value {
    serde_json::json!({
        "waiter_ref": waiter,
        "liveness": "durable",
        "prompt": {"title": title},
        "requires": {
            "v": 1,
            "answer": {"fields": [{
                "name": "decision", "type": "choice", "required": true,
                "options": [{"id": "approve", "label": "Approve"}, {"id": "reject", "label": "Reject"}]
            }]},
            "capabilities": [],
            "authority": {"min_role": "editor", "auth_strength": "session"}
        }
    })
}
