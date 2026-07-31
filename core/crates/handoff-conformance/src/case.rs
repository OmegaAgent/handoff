//! The declarative case format.
//!
//! A case file is the normative, human-readable statement of one conformance test. The Rust types
//! here are a transcription of that format and nothing more: no case-specific behaviour lives in
//! this crate, so a reader can check a case against the specification without reading Rust.
//!
//! The format is documented for implementers in `CASE-FORMAT.md` next to this crate.

use serde::Deserialize;
use std::collections::BTreeMap;

/// One conformance case, loaded from a single YAML file under `conformance/cases/`.
#[derive(Debug, Clone, Deserialize)]
pub struct Case {
    /// Stable identifier, exactly as it appears in specification §18 — `C-3`, `C-6b`, `C-22`.
    ///
    /// These are referenced by published conformance runs and MUST NOT be renumbered.
    pub id: String,

    /// One line a person can read without the specification open.
    pub title: String,

    /// Conformance level this case belongs to: `1` is required of every Server, `2` is the
    /// optional `continuation` extension of §14.
    pub level: u8,

    /// The invariants from §17 this case proves, by number — `I5`, `I17`.
    pub invariants: Vec<String>,

    /// Sections of the specification this case is derived from, for a reader tracing it back.
    #[serde(default)]
    pub spec: Vec<String>,

    /// Why the case exists: the failure it is designed to catch, in prose.
    #[serde(default)]
    pub rationale: Option<String>,

    /// What the deployment under test must supply before this case can run at all.
    #[serde(default)]
    pub requires: Requirements,

    /// The steps, in order. A case fails on its first failing step.
    pub steps: Vec<Step>,
}

/// What a deployment must supply for a case to be executable.
///
/// A missing requirement is a **failure**, never a skip. A suite that quietly skips the cases a
/// deployment cannot satisfy reports conformance it did not measure.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Requirements {
    /// Principal aliases this case authenticates as, resolved from the deployment profile.
    #[serde(default)]
    pub principals: Vec<String>,

    /// Deployment-supplied hooks this case invokes — see [`Action::Hook`].
    #[serde(default)]
    pub hooks: Vec<String>,

    /// A directory of fixture files, relative to the repository's `spec/` directory.
    #[serde(default)]
    pub fixtures: Option<String>,
}

/// One step. Exactly one action field is populated; `name` is what the report prints.
#[derive(Debug, Clone, Deserialize)]
pub struct Step {
    /// What this step does, in the words of the specification.
    pub name: String,

    /// Run this step only when a deployment-profile flag holds. Used by C-6b, where the correct
    /// behaviour differs between a deployment that permits `link_only` and one that forbids it.
    #[serde(default)]
    pub when: Option<When>,

    /// The action itself.
    #[serde(flatten)]
    pub action: Action,
}

/// A profile-flag predicate guarding a step.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct When {
    /// Name of a boolean flag under `deployment:` in the profile.
    pub flag: String,
    /// The value the flag must hold for the step to run.
    pub is: bool,
}

/// The closed set of things a step can do.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    /// One HTTP call against the base URL, with expectations on the response.
    Http(HttpCall),

    /// Several HTTP calls released **simultaneously** from separate threads, with expectations on
    /// the multiset of responses. This is how C-3 tests a race rather than a sequence.
    Concurrent(Concurrent),

    /// Repeat an HTTP call until its expectations hold or the timeout elapses. For sweeps that a
    /// Server performs on its own clock — an attempt lapse, a TTL expiry.
    Poll(Poll),

    /// Open a long poll and drop the connection without reading the response, simulating a client
    /// process dying mid-wait (C-11).
    AbandonPoll(AbandonPoll),

    /// Move the clock forward. Uses the deployment's `advance_clock` hook when it has one;
    /// otherwise sleeps for real, and fails when the duration exceeds `max_real_sleep`.
    AdvanceClock(AdvanceClock),

    /// Search every artifact the scenario produced for a literal string, and require zero hits.
    /// C-7 and C-8 are scans, not unit tests.
    Scan(Scan),

    /// Walk every JSON body seen in this case and require that no object carries a forbidden key.
    /// This is how C-22 asserts the absence of a request-kind discriminator on the wire.
    ForbidKeys(ForbidKeys),

    /// Invoke a deployment-supplied command. The runner speaks only HTTP; anything below the API —
    /// a log dump (C-7), an inbound channel message (C-21) — is a hook the deployment provides and
    /// this suite merely calls.
    Hook(Hook),

    /// Read the tenant's receipts over HTTP and **recompute the chain here**, then require the
    /// deployment's exported head to be the head that walk arrived at (C-15, C-26).
    VerifyChain(VerifyChain),

    /// Attempt a mutation below the API through a deployment hook, and decide the step on what the
    /// HTTP surface shows afterwards rather than on what the hook said (C-15).
    StorageMutation(StorageMutation),

    /// Bind a local HTTP listener and expose its URL, so callbacks the Server sends can be captured
    /// (C-18).
    CallbackReceiver(CallbackReceiver),

    /// Assert over the callbacks captured so far.
    CallbackAssert(CallbackAssert),

    /// Run a block of steps once per fixture file in `requires.fixtures`, binding the file's
    /// contents to `${fixture_body}` and its stem to `${fixture_name}` (C-22).
    ForEachFixture(ForEachFixture),
}

/// One HTTP call and what must be true of its response.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HttpCall {
    /// HTTP method, uppercase.
    pub method: String,

    /// Path below the base URL, starting with `/`. `${var}` interpolation applies.
    pub path: String,

    /// Principal alias to authenticate as. `anonymous` sends no credential at all.
    #[serde(default = "default_principal")]
    pub r#as: String,

    /// Extra request headers. `${var}` interpolation applies to values.
    #[serde(default)]
    pub headers: BTreeMap<String, String>,

    /// JSON request body. Strings inside it are `${var}`-interpolated.
    #[serde(default)]
    pub body: Option<serde_json::Value>,

    /// Query parameters, appended in the order given.
    #[serde(default)]
    pub query: BTreeMap<String, String>,

    /// What must be true of the response.
    #[serde(default)]
    pub expect: Expect,

    /// Response-body values to bind as variables for later steps: `{var_name: json_path}`.
    #[serde(default)]
    pub capture: BTreeMap<String, String>,

    /// Response headers to bind as variables: `{var_name: Header-Name}`.
    #[serde(default)]
    pub capture_headers: BTreeMap<String, String>,
}

fn default_principal() -> String {
    "machine_a".to_string()
}

/// Expectations on one HTTP response.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Expect {
    /// Exact HTTP status.
    #[serde(default)]
    pub status: Option<u16>,

    /// Any one of these statuses. Use only where the specification genuinely permits a choice.
    #[serde(default)]
    pub status_in: Vec<u16>,

    /// Assertions against the response body.
    #[serde(default)]
    pub body: Vec<Matcher>,

    /// Assertions against response headers, keyed by header name.
    #[serde(default)]
    pub headers: Vec<Matcher>,
}

/// One assertion: a path into the JSON, an operator, and why it matters.
#[derive(Debug, Clone, Deserialize)]
pub struct Matcher {
    /// Dotted path with optional indices — `error.code`, `data[0].id`, `data[].org_id`. An empty
    /// path (`""`) addresses the whole document.
    #[serde(default)]
    pub path: String,

    /// The comparison to perform.
    #[serde(flatten)]
    pub op: Op,

    /// The specification sentence this assertion enforces. Printed on failure, so a red case
    /// explains itself without the reader opening the spec.
    #[serde(default)]
    pub because: Option<String>,
}

/// The closed set of comparisons a matcher can make.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Op {
    /// Value at the path equals this JSON value exactly.
    Equals(serde_json::Value),
    /// Value at the path differs from this JSON value.
    NotEquals(serde_json::Value),
    /// The path resolves (`true`) or does not resolve (`false`).
    Exists(bool),
    /// The value is JSON `null` (`true`) or is not (`false`).
    IsNull(bool),
    /// The value is a string matching this regular expression.
    Matches(String),
    /// The value is an array or string of exactly this length.
    Length(usize),
    /// The value is an array or string of at least this length.
    LengthAtLeast(usize),
    /// The value equals one of these.
    OneOf(Vec<serde_json::Value>),
    /// The value equals a previously captured variable.
    SameAs(String),
    /// The value differs from a previously captured variable. C-8 uses this to prove two resolves
    /// mint two different URLs.
    DiffersFrom(String),
    /// Every value produced by a wildcard path equals this. C-20 uses it to assert tenant identity
    /// across a whole page.
    AllEqual(serde_json::Value),
    /// No value produced by a wildcard path equals this.
    NoneEqual(serde_json::Value),
    /// The values produced by a wildcard path are exactly this set — length **and** identity.
    /// §18 is explicit that `contains` is the wrong assertion for tenant isolation, because a query
    /// missing its tenant predicate returns a superset and passes it.
    SetEquals(Vec<String>),
    /// The document, serialized, contains this substring.
    ContainsText(String),
    /// The document, serialized, does not contain this substring.
    NotContainsText(String),
}

/// A set of HTTP calls released at the same instant.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Concurrent {
    /// The calls. Every one is prepared fully, then all are released together off a barrier, so
    /// they contend rather than queue.
    pub calls: Vec<HttpCall>,

    /// A partition of the responses. Every outcome must be matched exactly `count` times, and
    /// every response must be claimed by exactly one outcome.
    pub expect_outcomes: Vec<Outcome>,
}

/// One class of response in a concurrent step, with how many responses must fall into it.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Outcome {
    /// How many of the concurrent responses must match this class.
    pub count: usize,
    /// Required HTTP status.
    pub status: u16,
    /// Required body assertions.
    #[serde(default)]
    pub body: Vec<Matcher>,
    /// What this outcome proves.
    #[serde(default)]
    pub because: Option<String>,
}

/// Repeat a call until it satisfies its expectations.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Poll {
    /// The call to repeat.
    pub call: HttpCall,
    /// Give up after this ISO-8601 duration and fail.
    pub timeout: String,
    /// Wait this long between attempts.
    #[serde(default = "default_interval")]
    pub interval: String,
}

fn default_interval() -> String {
    "PT1S".to_string()
}

/// Open a long poll and abandon it.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AbandonPoll {
    /// Path to poll.
    pub path: String,
    /// Principal alias.
    #[serde(default = "default_principal")]
    pub r#as: String,
    /// Query parameters — typically `wait`.
    #[serde(default)]
    pub query: BTreeMap<String, String>,
    /// Hold the connection this long before dropping it.
    #[serde(default = "default_abandon_after")]
    pub abandon_after: String,
}

fn default_abandon_after() -> String {
    "PT1S".to_string()
}

/// Move the clock forward, by hook where one exists and by real time where it does not.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdvanceClock {
    /// ISO-8601 duration to advance.
    pub by: String,
}

/// Where a scan looks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScanSource {
    /// Every request and response body recorded in this case.
    Traffic,
    /// Every URL, including query strings, that this case sent.
    Urls,
    /// Every request and response **header** recorded in this case.
    Headers,
    /// A freshly fetched `GET /requests/{id}`.
    Request,
    /// A freshly fetched `GET /requests/{id}/receipt`.
    Receipt,
    /// A freshly fetched `GET /requests/{id}/deliveries`.
    Deliveries,
    /// A freshly fetched `GET /waiters/{waiter_ref}/signals`.
    Signals,
    /// The output of the deployment's `logs` hook.
    Logs,
}

/// Search artifacts for strings that must not appear anywhere.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scan {
    /// The literal strings to look for, after `${var}` interpolation.
    pub r#for: Vec<String>,
    /// Where to look. Every named source must be searchable, or the step fails.
    pub r#in: Vec<ScanSource>,
    /// Request id for the `request`, `receipt`, and `deliveries` sources.
    #[serde(default)]
    pub request: Option<String>,
    /// Waiter reference for the `signals` source.
    #[serde(default)]
    pub waiter_ref: Option<String>,
    /// Principal to read the fetched sources as.
    #[serde(default = "default_principal")]
    pub r#as: String,
    /// What a hit would mean.
    #[serde(default)]
    pub because: Option<String>,
}

/// Require that no JSON object anywhere in the recorded traffic carries certain keys.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForbidKeys {
    /// Object keys that must not appear.
    pub keys: Vec<String>,
    /// Dotted path prefixes where the key is legitimate and therefore permitted. `Evidence.kind`
    /// and `Target.kind` are real protocol fields; a request-level `kind` discriminator is not.
    #[serde(default)]
    pub allow_paths: Vec<String>,
    /// What a hit would mean.
    #[serde(default)]
    pub because: Option<String>,
}

/// Invoke a deployment-supplied command.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Hook {
    /// Hook name, looked up under `hooks:` in the deployment profile.
    pub hook: String,
    /// Arguments, exported to the command as `HANDOFF_ARG_<UPPERCASE_KEY>` after interpolation.
    #[serde(default)]
    pub args: BTreeMap<String, String>,
    /// Expectations on the command's result.
    #[serde(default)]
    pub expect: HookExpect,
    /// Capture stdout as a variable.
    #[serde(default)]
    pub capture_stdout: Option<String>,
}

/// What must be true of a hook invocation.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HookExpect {
    /// The command must exit with exactly this code.
    #[serde(default)]
    pub exit_code: Option<i32>,
    /// The command must **not** exit with this code. `exit_code_not: 0` is how C-15 asserts that
    /// the storage engine refused a mutation.
    #[serde(default)]
    pub exit_code_not: Option<i32>,
    /// Regular expressions the combined stdout and stderr must match.
    #[serde(default)]
    pub output_matches: Vec<String>,
    /// Regular expressions the combined stdout and stderr must **not** match.
    ///
    /// This exists because the runner's `regex` crate has no look-around, so "the output does not
    /// contain X" cannot be written as a negative lookahead inside `output_matches`. Expressing the
    /// negation as its own field is also plainer to read than a lookahead would have been.
    #[serde(default)]
    pub output_not_matches: Vec<String>,
    /// What the expectation proves.
    #[serde(default)]
    pub because: Option<String>,
}

/// Walk the receipt chain in the suite's own implementation of `signing.md` §2.2.
///
/// This action asks the deployment for nothing but receipts. It reads the listing, recomputes every
/// `chain.digest` from the receipt's own content, requires the exported head to be the head that
/// walk arrived at, and then rewrites each receipt in turn and requires the walk to break — the
/// guard that keeps a green walk from being a walk over nothing.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerifyChain {
    /// Principal alias to read as.
    #[serde(default = "default_principal")]
    pub r#as: String,

    /// Path of the receipt listing — `/receipts`. Written in the case rather than assumed, because
    /// every other path in this format is.
    pub receipts: String,

    /// Path of the exported head — `/receipts/chain-head`. The walk must arrive at what it serves.
    pub head: String,

    /// Receipt ids that must appear in the walk. A chain the case's own receipt is missing from is
    /// not the chain the case is talking about.
    #[serde(default)]
    pub must_include: Vec<String>,

    /// Fail unless the walk covers at least this many receipts.
    #[serde(default)]
    pub at_least: usize,

    /// Receipt ids that must also verify **on their own**, from the receipt's own stored
    /// `chain.prev_digest` rather than from a walk.
    ///
    /// §2.2 stores the predecessor's digest on every receipt rather than implying it, so that a
    /// party holding one receipt — an auditor, a customer after they have left — can verify it
    /// without being handed the chain. That is a different claim from "the chain verifies", and
    /// nothing else in the suite makes it.
    #[serde(default)]
    pub standalone: Vec<String>,

    /// What the walk proves.
    #[serde(default)]
    pub because: Option<String>,
}

/// What a storage-level mutation attempt must have done.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MutationOutcome {
    /// The storage engine refused it: the command failed, and the object is byte-identical
    /// afterwards to what the suite read before the attempt.
    Refused,
    /// The storage engine applied it: the command succeeded, and the HTTP surface now shows the
    /// change. This is the positive control — the same command, aimed at a row the engine permits,
    /// proving the hook reaches the store this API serves from.
    Applied,
}

/// Attempt a mutation below the HTTP API, and judge it by what HTTP shows afterwards.
///
/// §9.4 puts the application inside the threat model, so C-15 cannot ask the API to mutate a
/// receipt: the attempt must be made against the storage engine directly, and only the deployment
/// can do that. What the *suite* can do is refuse to take the hook's word for the outcome. It reads
/// the object over HTTP before and after, and the step is decided by the difference.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StorageMutation {
    /// Hook name, looked up under `hooks:` in the deployment profile.
    pub hook: String,

    /// Arguments, exported as `HANDOFF_ARG_<UPPERCASE_KEY>` after interpolation.
    #[serde(default)]
    pub args: BTreeMap<String, String>,

    /// Whether the engine must have refused this mutation or applied it.
    pub expect: MutationOutcome,

    /// Regular expressions the command's output must match — the engine's own refusal, typically.
    /// Secondary evidence: the assertion that decides the step is the observation below.
    #[serde(default)]
    pub output_matches: Vec<String>,

    /// What to read over HTTP, before and after.
    pub observe: Observation,

    /// What the outcome proves.
    #[serde(default)]
    pub because: Option<String>,
}

/// The HTTP read that decides a [`StorageMutation`].
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Observation {
    /// Path to read, before the attempt and again after it.
    pub path: String,

    /// Principal alias to read as.
    #[serde(default = "default_principal")]
    pub r#as: String,

    /// Assertions against the body read **after** the attempt.
    #[serde(default)]
    pub body: Vec<Matcher>,
}

/// Start capturing callbacks on a local listener.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CallbackReceiver {
    /// Variable to bind the listener's URL to, for use in a raise body's `callback.url`.
    pub bind_url_to: String,
    /// HTTP status the listener returns. `200` is the interesting case: a `2xx` marks a callback
    /// dispatched and MUST NOT consume the signal (§15.4).
    #[serde(default = "default_callback_status")]
    pub respond_status: u16,
}

fn default_callback_status() -> u16 {
    200
}

/// The closed set of assertions over captured callbacks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CallbackCheck {
    /// Every callback carries a signature, a version, a timestamp, and a sequence (§15.1).
    Signed,
    /// `sequence` is monotonically increasing per waiter (§8.3).
    SequenceMonotonicPerWaiter,
    /// The signature over a captured callback verifies against the profile's current secret.
    SignatureVerifies,
    /// The same signature replayed onto a different delivery does not verify — that is, the signed
    /// string binds the delivery, not only the body.
    ReplayOntoOtherDeliveryRejected,
    /// Altering the body by one byte breaks verification.
    OneByteTamperRejected,
    /// A timestamp outside the freshness window is refused.
    StaleTimestampRejected,
    /// During a rotation overlap, callbacks signed with either secret verify.
    RotationOverlapBothVerify,
    /// A `2xx` from the receiver does not stop redelivery; only an ack does (§15.4, §8.3).
    RedeliversUntilAcked,
    /// No callback carries a capability, a resolvable URL, or a secret value (§15.6).
    CarriesNoResolvableUrl,
    /// The body carries no tenant identifier, so a receiver cannot resolve tenancy from it (§15.3,
    /// I13). A valid signature proves the sender; it never proves the tenant.
    TenancyNotDerivableFromBody,
}

/// Assertions over the callbacks captured so far.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CallbackAssert {
    /// Wait up to this long for the expected number of callbacks to arrive.
    #[serde(default = "default_callback_timeout")]
    pub timeout: String,
    /// Minimum number of callbacks that must have arrived.
    #[serde(default)]
    pub at_least: usize,
    /// The checks to run.
    pub checks: Vec<CallbackCheck>,
    /// What the checks prove.
    #[serde(default)]
    pub because: Option<String>,
}

fn default_callback_timeout() -> String {
    "PT30S".to_string()
}

/// Run a block of steps once per fixture file.
///
/// Three variables are bound for each file: `${fixture_name}` (the file stem), `${fixture_body}`
/// (the whole document) and `${fixture_raise}` (the document's `raise` member, or the whole
/// document when it has none).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForEachFixture {
    /// Steps to run for each fixture.
    pub steps: Vec<Step>,
    /// Fail unless at least this many fixture files were found. §5.6 defines eight patterns.
    #[serde(default)]
    pub at_least: usize,
    /// Run the block only for fixtures whose named member is present and not null, and record the
    /// rest as excluded. §5.6 says two of the eight patterns deliberately resolve to "not a request
    /// shape", so a suite that insisted on raising all eight would be asserting the opposite of
    /// what the specification says.
    #[serde(default)]
    pub when_present: Option<String>,
    /// Exactly this many fixtures are expected to be excluded by `when_present`. Stated so the
    /// exclusion is a claim the case makes rather than a silent filter.
    #[serde(default)]
    pub expect_excluded: Option<usize>,
}

/// Parse a case file, reporting the file name on a syntax error.
pub fn parse(path: &std::path::Path, text: &str) -> Result<Case, String> {
    serde_yaml::from_str::<Case>(text).map_err(|e| format!("{}: {e}", path.display()))
}
