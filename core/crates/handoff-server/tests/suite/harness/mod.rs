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
    /// Create a database, seed credentials, and start `handoffd`.
    pub async fn start(label: &str, port: u16) -> Deployment {
        Self::start_with(label, port, &[]).await
    }

    /// The same, with extra environment for the server process.
    ///
    /// Callback signing is the case this exists for: `HANDOFF_CALLBACK_SECRETS` is deployment
    /// configuration, and a test about rotation needs two of them, which no default can supply.
    pub async fn start_with(label: &str, port: u16, env: &[(&str, &str)]) -> Deployment {
        let database = format!(
            "handoff_test_{label}_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or_default()
        );
        let url = database_url(&database);

        psql(
            &admin_url(),
            &format!("drop database if exists \"{database}\" with (force)"),
        );
        psql(&admin_url(), &format!("create database \"{database}\""));

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

        for _ in 0..200 {
            if reqwest::get(format!("{}/meta", self.base)).await.is_ok() {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        panic!("handoffd never answered on port {}", self.port);
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
    pub async fn pool(&self) -> sqlx::PgPool {
        sqlx::postgres::PgPoolOptions::new()
            .max_connections(4)
            .connect(&self.url)
            .await
            .expect("connect to the test database")
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
