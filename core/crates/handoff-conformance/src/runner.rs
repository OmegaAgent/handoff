//! Step execution.
//!
//! A case runs its steps in order and stops at the first failure, carrying the step name and a
//! reason. Nothing here knows which case it is running: every case-specific fact lives in the YAML,
//! so the suite can be audited by reading `conformance/cases/` rather than this file.

use crate::callback::Receiver;
use crate::case::{
    Action, CallbackAssert, CallbackReceiver, Case, Concurrent, ForEachFixture, ForbidKeys, Hook,
    HttpCall, Matcher, Poll, Scan, ScanSource, Step,
};
use crate::expect;
use crate::http::{Client, Response};
use crate::profile::Profile;
use crate::vars;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Barrier};

/// How one case came out.
#[derive(Debug, Clone)]
pub struct CaseResult {
    /// Stable case id from §18.
    pub id: String,
    /// Human title.
    pub title: String,
    /// Conformance level.
    pub level: u8,
    /// Invariants this case proves.
    pub invariants: Vec<String>,
    /// The step that failed and why, or `None` when the case passed.
    pub failure: Option<Failure>,
}

impl CaseResult {
    /// Whether the case passed.
    pub fn passed(&self) -> bool {
        self.failure.is_none()
    }
}

/// A failed step.
#[derive(Debug, Clone)]
pub struct Failure {
    /// Which step, by its `name`.
    pub step: String,
    /// What went wrong, in terms a reader can act on.
    pub reason: String,
}

/// Everything a case needs while it runs.
pub struct Runner<'a> {
    client: Client,
    profile: &'a Profile,
    repo_root: PathBuf,
}

struct Scope {
    vars: BTreeMap<String, String>,
    receivers: Vec<Receiver>,
}

impl<'a> Runner<'a> {
    /// Build a runner against a base URL.
    pub fn new(base_url: &str, profile: &'a Profile, repo_root: PathBuf) -> Self {
        Self {
            client: Client::new(base_url),
            profile,
            repo_root,
        }
    }

    /// Run one case start to finish.
    pub fn run(&self, case: &Case) -> CaseResult {
        self.client.clear_traffic();
        let mut scope = Scope {
            vars: BTreeMap::new(),
            receivers: Vec::new(),
        };
        scope.vars.insert("run_id".to_string(), vars::run_id());
        scope.vars.insert(
            "case_id".to_string(),
            case.id.to_lowercase().replace('-', ""),
        );
        scope
            .vars
            .insert("base_url".to_string(), self.client.base_url().to_string());

        let failure = self
            .check_requirements(case)
            .err()
            .map(|reason| Failure {
                step: "preconditions".to_string(),
                reason,
            })
            .or_else(|| self.run_steps(&case.steps, case, &mut scope).err());

        CaseResult {
            id: case.id.clone(),
            title: case.title.clone(),
            level: case.level,
            invariants: case.invariants.clone(),
            failure,
        }
    }

    fn check_requirements(&self, case: &Case) -> Result<(), String> {
        for alias in &case.requires.principals {
            self.profile.principal(alias)?;
        }
        for hook in &case.requires.hooks {
            self.profile.hook(hook)?;
        }
        if let Some(dir) = &case.requires.fixtures {
            let path = self.fixtures_dir(dir);
            if !path.is_dir() {
                return Err(format!(
                    "{} does not exist. The specification cites it as normative for conformance \
                     (§5.6, C-22), so the case cannot run until the fixtures are published.",
                    path.display()
                ));
            }
        }
        Ok(())
    }

    fn fixtures_dir(&self, dir: &str) -> PathBuf {
        self.repo_root.join(&self.profile.fixtures_root).join(dir)
    }

    fn run_steps(&self, steps: &[Step], case: &Case, scope: &mut Scope) -> Result<(), Failure> {
        for step in steps {
            if let Some(when) = &step.when {
                match self.profile.flag(&when.flag) {
                    Ok(actual) if actual != when.is => continue,
                    Ok(_) => {}
                    Err(reason) => {
                        return Err(Failure {
                            step: step.name.clone(),
                            reason,
                        });
                    }
                }
            }
            if let Err(reason) = self.run_action(&step.action, case, scope) {
                return Err(Failure {
                    step: step.name.clone(),
                    reason,
                });
            }
        }
        Ok(())
    }

    fn run_action(&self, action: &Action, case: &Case, scope: &mut Scope) -> Result<(), String> {
        match action {
            Action::Http(call) => self.do_http(call, scope).map(|_| ()),
            Action::Concurrent(c) => self.do_concurrent(c, scope),
            Action::Poll(p) => self.do_poll(p, scope),
            Action::AbandonPoll(a) => {
                let principal = self.profile.principal(&a.r#as)?;
                let path = vars::interpolate(&a.path, &scope.vars)?;
                let query = self.fill_map(&a.query, scope)?;
                let hold = vars::parse_duration(&a.abandon_after)?;
                self.client.abandon(&path, &query, &principal, hold)
            }
            Action::AdvanceClock(a) => self.do_advance_clock(&a.by, scope),
            Action::Scan(s) => self.do_scan(s, scope),
            Action::ForbidKeys(f) => self.do_forbid_keys(f),
            Action::Hook(h) => self.do_hook(h, scope),
            Action::CallbackReceiver(r) => self.do_callback_receiver(r, scope),
            Action::CallbackAssert(a) => self.do_callback_assert(a, scope),
            Action::ForEachFixture(f) => self.do_for_each_fixture(f, case, scope),
        }
    }

    // ---------------------------------------------------------------- http

    fn fill_map(
        &self,
        map: &BTreeMap<String, String>,
        scope: &Scope,
    ) -> Result<BTreeMap<String, String>, String> {
        map.iter()
            .map(|(k, v)| Ok((k.clone(), vars::interpolate(v, &scope.vars)?)))
            .collect()
    }

    fn send(&self, call: &HttpCall, scope: &Scope) -> Result<Response, String> {
        let principal = self.profile.principal(&call.r#as)?;
        let path = vars::interpolate(&call.path, &scope.vars)?;
        let headers = self.fill_map(&call.headers, scope)?;
        let query = self.fill_map(&call.query, scope)?;
        let body = match &call.body {
            Some(b) => Some(vars::interpolate_json(b, &scope.vars)?),
            None => None,
        };
        self.client.call(
            &call.method,
            &path,
            &query,
            &headers,
            body.as_ref(),
            &principal,
        )
    }

    fn do_http(&self, call: &HttpCall, scope: &mut Scope) -> Result<Response, String> {
        let response = self.send(call, scope)?;
        self.assert_response(call, &response, &scope.vars)?;
        self.capture(call, &response, scope)?;
        Ok(response)
    }

    fn assert_response(
        &self,
        call: &HttpCall,
        response: &Response,
        vars: &BTreeMap<String, String>,
    ) -> Result<(), String> {
        let context = || {
            let body = response.body.trim();
            if body.is_empty() {
                "<empty body>".to_string()
            } else {
                let head: String = body.chars().take(400).collect();
                head
            }
        };
        if let Some(want) = call.expect.status {
            if response.status != want {
                return Err(format!(
                    "{} {} returned {} but the specification requires {want}\n      body: {}",
                    call.method,
                    call.path,
                    response.status,
                    context()
                ));
            }
        }
        if !call.expect.status_in.is_empty() && !call.expect.status_in.contains(&response.status) {
            return Err(format!(
                "{} {} returned {} but the specification permits only {:?}\n      body: {}",
                call.method,
                call.path,
                response.status,
                call.expect.status_in,
                context()
            ));
        }
        check_all(&response.json(), &call.expect.body, vars)?;
        check_all(&response.headers_json(), &call.expect.headers, vars)
    }

    fn capture(
        &self,
        call: &HttpCall,
        response: &Response,
        scope: &mut Scope,
    ) -> Result<(), String> {
        let doc = response.json();
        for (name, path) in &call.capture {
            let hits = expect::resolve(&doc, path);
            let hit = hits.first().ok_or_else(|| {
                format!("cannot capture `{name}`: `{path}` is absent from the response")
            })?;
            scope.vars.insert(name.clone(), text_of(&hit.value));
        }
        for (name, header) in &call.capture_headers {
            let value = response
                .headers
                .get(&header.to_lowercase())
                .ok_or_else(|| {
                    format!("cannot capture `{name}`: no `{header}` header on the response")
                })?;
            scope.vars.insert(name.clone(), value.clone());
        }
        Ok(())
    }

    // ---------------------------------------------------------------- concurrency

    fn do_concurrent(&self, c: &Concurrent, scope: &mut Scope) -> Result<(), String> {
        // Everything that can be prepared is prepared before the barrier, so what contends is the
        // Server's settling write and not our own string formatting. C-3 is about a race; a suite
        // that serializes the calls proves nothing about first-writer-wins.
        struct Prepared {
            method: String,
            path: String,
            headers: BTreeMap<String, String>,
            query: BTreeMap<String, String>,
            body: Option<serde_json::Value>,
            principal: crate::profile::Principal,
        }

        let mut prepared = Vec::with_capacity(c.calls.len());
        for call in &c.calls {
            prepared.push(Prepared {
                method: call.method.clone(),
                path: vars::interpolate(&call.path, &scope.vars)?,
                headers: self.fill_map(&call.headers, scope)?,
                query: self.fill_map(&call.query, scope)?,
                body: match &call.body {
                    Some(b) => Some(vars::interpolate_json(b, &scope.vars)?),
                    None => None,
                },
                principal: self.profile.principal(&call.r#as)?,
            });
        }

        let barrier = Arc::new(Barrier::new(prepared.len()));
        let mut handles = Vec::with_capacity(prepared.len());
        for p in prepared {
            let barrier = Arc::clone(&barrier);
            let client = self.client.clone();
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                client.call(
                    &p.method,
                    &p.path,
                    &p.query,
                    &p.headers,
                    p.body.as_ref(),
                    &p.principal,
                )
            }));
        }

        let mut responses = Vec::with_capacity(handles.len());
        for (i, handle) in handles.into_iter().enumerate() {
            match handle.join() {
                Ok(Ok(r)) => responses.push(r),
                Ok(Err(e)) => return Err(format!("concurrent call {i} failed: {e}")),
                Err(_) => return Err(format!("concurrent call {i} panicked")),
            }
        }

        let mut unclaimed: Vec<usize> = (0..responses.len()).collect();
        for outcome in &c.expect_outcomes {
            let mut claimed = Vec::new();
            for &i in &unclaimed {
                if responses[i].status != outcome.status {
                    continue;
                }
                if check_all(&responses[i].json(), &outcome.body, &scope.vars).is_ok() {
                    claimed.push(i);
                }
                if claimed.len() == outcome.count {
                    break;
                }
            }
            if claimed.len() != outcome.count {
                let seen: Vec<String> = responses
                    .iter()
                    .map(|r| format!("{} {}", r.status, first_line(&r.body)))
                    .collect();
                let because = outcome
                    .because
                    .as_deref()
                    .map(|w| format!("\n      because: {w}"))
                    .unwrap_or_default();
                return Err(format!(
                    "expected exactly {} response(s) with status {} matching the stated body, \
                     found {}\n      responses: {seen:?}{because}",
                    outcome.count,
                    outcome.status,
                    claimed.len()
                ));
            }
            unclaimed.retain(|i| !claimed.contains(i));
        }
        if !unclaimed.is_empty() {
            let extra: Vec<String> = unclaimed
                .iter()
                .map(|&i| format!("{} {}", responses[i].status, first_line(&responses[i].body)))
                .collect();
            return Err(format!(
                "{} concurrent response(s) matched no declared outcome: {extra:?}",
                unclaimed.len()
            ));
        }
        Ok(())
    }

    // ---------------------------------------------------------------- polling and clocks

    fn do_poll(&self, p: &Poll, scope: &mut Scope) -> Result<(), String> {
        let timeout = vars::parse_duration(&p.timeout)?;
        let interval = vars::parse_duration(&p.interval)?;
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let outcome = match self.send(&p.call, scope) {
                Ok(response) => match self.assert_response(&p.call, &response, &scope.vars) {
                    Ok(()) => return self.capture(&p.call, &response, scope),
                    Err(reason) => reason,
                },
                Err(reason) => reason,
            };
            if std::time::Instant::now() >= deadline {
                return Err(format!(
                    "still not satisfied after {}: {outcome}",
                    p.timeout
                ));
            }
            std::thread::sleep(interval);
        }
    }

    fn do_advance_clock(&self, by: &str, scope: &Scope) -> Result<(), String> {
        let by = vars::interpolate(by, &scope.vars)?;
        let duration = vars::parse_duration(&by)?;
        match self.profile.hooks.get("advance_clock") {
            Some(command) => {
                let args = BTreeMap::from([("by".to_string(), by.clone())]);
                let (code, output) = run_command(command, &args)?;
                if code == 0 {
                    Ok(())
                } else {
                    Err(format!("`advance_clock` hook exited {code}: {output}"))
                }
            }
            None => {
                let ceiling = vars::parse_duration(&self.profile.max_real_sleep)?;
                if duration > ceiling {
                    return Err(format!(
                        "advancing {by} needs an `advance_clock` hook: waiting for it in real time \
                         exceeds max_real_sleep ({}). Supply the hook, or configure the deployment \
                         with a shorter sweep interval.",
                        self.profile.max_real_sleep
                    ));
                }
                std::thread::sleep(duration);
                Ok(())
            }
        }
    }

    // ---------------------------------------------------------------- scans

    fn do_scan(&self, s: &Scan, scope: &Scope) -> Result<(), String> {
        let needles: Vec<String> = s
            .r#for
            .iter()
            .map(|n| vars::interpolate(n, &scope.vars))
            .collect::<Result<_, _>>()?;
        for needle in &needles {
            if needle.trim().is_empty() {
                return Err("case defect: scanning for an empty string proves nothing".to_string());
            }
        }

        let mut corpus: Vec<(String, String)> = Vec::new();
        for source in &s.r#in {
            match source {
                // Response bodies only, deliberately.
                //
                // The scan hunts for values a *Server* leaked into an artifact. A request body is
                // the suite's own outbound bytes, and C-7 is required to send a secret to the sink
                // endpoint, because §12.3 makes that the one path a value legitimately travels.
                // Including request bodies therefore made C-7 find the secret it had just sent on
                // purpose, and no conforming Server could ever pass it.
                //
                // The client-side placements that *are* forbidden — a URL, a query string, a path,
                // a header (I18) — are covered by the `urls` and `headers` sources, which do read
                // the request side. Nothing is lost by excluding the body here.
                ScanSource::Traffic => {
                    for ex in self.client.traffic() {
                        corpus.push((
                            format!("response body of {} {}", ex.method, ex.url),
                            ex.response_body.clone(),
                        ));
                    }
                }
                ScanSource::Urls => {
                    for ex in self.client.traffic() {
                        corpus.push((format!("URL of {}", ex.method), ex.url.clone()));
                    }
                }
                ScanSource::Headers => {
                    for ex in self.client.traffic() {
                        for (k, v) in &ex.request_headers {
                            corpus.push((format!("request header {k} on {}", ex.url), v.clone()));
                        }
                        for (k, v) in &ex.response_headers {
                            corpus.push((format!("response header {k} on {}", ex.url), v.clone()));
                        }
                    }
                }
                ScanSource::Request | ScanSource::Receipt | ScanSource::Deliveries => {
                    let id = s.request.as_deref().ok_or_else(|| {
                        format!("scanning `{source:?}` needs a `request:` id on the step")
                    })?;
                    let id = vars::interpolate(id, &scope.vars)?;
                    let suffix = match source {
                        ScanSource::Receipt => "/receipt",
                        ScanSource::Deliveries => "/deliveries",
                        _ => "",
                    };
                    let path = format!("/requests/{id}{suffix}");
                    let body = self.fetch(&path, &s.r#as)?;
                    corpus.push((format!("GET {path}"), body));
                }
                ScanSource::Signals => {
                    let waiter = s.waiter_ref.as_deref().ok_or_else(|| {
                        "scanning `signals` needs a `waiter_ref:` on the step".to_string()
                    })?;
                    let waiter = vars::interpolate(waiter, &scope.vars)?;
                    let path = format!("/waiters/{}/signals", percent(&waiter));
                    let body = self.fetch(&path, &s.r#as)?;
                    corpus.push((format!("GET {path}"), body));
                }
                // The one source that can fail to produce anything and still leave the scan
                // looking healthy, because seven other sources contribute. C-7's rationale is
                // explicit that a deployment which cannot show its logs has not demonstrated the
                // property, so both a failing hook and a silent one are failures of the case —
                // not a source that quietly contributes nothing.
                ScanSource::Logs => {
                    let command = self.profile.hook("logs")?;
                    let (code, output) = run_command(&command, &BTreeMap::new())?;
                    if code != 0 {
                        return Err(format!(
                            "the `logs` hook exited {code}, so the deployment did not show its \
                             logs; §12.3 makes \"no secret in a log line\" normative and an \
                             unreadable log is a failure, not an exemption\n      output: {}",
                            head_lines(&output)
                        ));
                    }
                    if output.trim().is_empty() {
                        return Err(
                            "the `logs` hook produced no output, so scanning the logs found \
                             nothing because there was nothing to search; a deployment that \
                             cannot show its logs has not demonstrated §12.3"
                                .to_string(),
                        );
                    }
                    corpus.push(("the deployment's logs".to_string(), output));
                }
            }
        }

        if corpus.is_empty() {
            return Err("nothing was collected to scan; the scan proves nothing".to_string());
        }

        let mut hits = Vec::new();
        for (where_, text) in &corpus {
            for needle in &needles {
                if text.contains(needle.as_str()) {
                    hits.push(where_.clone());
                }
            }
        }
        if hits.is_empty() {
            Ok(())
        } else {
            hits.sort();
            hits.dedup();
            let because = s
                .because
                .as_deref()
                .map(|w| format!("\n      because: {w}"))
                .unwrap_or_default();
            Err(format!(
                "found the value in {} place(s), and it must appear in none: {hits:?}{because}",
                hits.len()
            ))
        }
    }

    fn fetch(&self, path: &str, alias: &str) -> Result<String, String> {
        let principal = self.profile.principal(alias)?;
        let response = self.client.call(
            "GET",
            path,
            &BTreeMap::new(),
            &BTreeMap::new(),
            None,
            &principal,
        )?;
        if response.status >= 400 {
            return Err(format!(
                "cannot read {path} for the scan: HTTP {} — the scan must inspect this artifact, \
                 so an unreadable one is a failure, not an exemption",
                response.status
            ));
        }
        Ok(response.body)
    }

    fn do_forbid_keys(&self, f: &ForbidKeys) -> Result<(), String> {
        let mut hits: Vec<String> = Vec::new();
        for ex in self.client.traffic() {
            let mut docs = Vec::new();
            if let Some(body) = &ex.request_body {
                docs.push(("request", body.clone()));
            }
            docs.push(("response", ex.response_body.clone()));
            for (side, raw) in docs {
                let Ok(doc) = serde_json::from_str::<serde_json::Value>(&raw) else {
                    continue;
                };
                let mut found = Vec::new();
                walk_keys(&doc, String::new(), &f.keys, &f.allow_paths, &mut found);
                for path in found {
                    hits.push(format!("{} {} {} at `{path}`", ex.method, ex.url, side));
                }
            }
        }
        if hits.is_empty() {
            Ok(())
        } else {
            hits.sort();
            hits.dedup();
            let because = f
                .because
                .as_deref()
                .map(|w| format!("\n      because: {w}"))
                .unwrap_or_default();
            Err(format!(
                "forbidden key(s) present in {} place(s): {hits:?}{because}",
                hits.len()
            ))
        }
    }

    // ---------------------------------------------------------------- hooks

    fn do_hook(&self, h: &Hook, scope: &mut Scope) -> Result<(), String> {
        let command = self.profile.hook(&h.hook)?;
        let args = self.fill_map(&h.args, scope)?;
        let (code, output) = run_command(&command, &args)?;

        let because = h
            .expect
            .because
            .as_deref()
            .map(|w| format!("\n      because: {w}"))
            .unwrap_or_default();
        if let Some(want) = h.expect.exit_code {
            if code != want {
                return Err(format!(
                    "hook `{}` exited {code}, expected {want}\n      output: {}{because}",
                    h.hook,
                    head_lines(&output)
                ));
            }
        }
        if let Some(forbidden) = h.expect.exit_code_not {
            if code == forbidden {
                return Err(format!(
                    "hook `{}` exited {code}, and it must not\n      output: {}{because}",
                    h.hook,
                    head_lines(&output)
                ));
            }
        }
        for pattern in &h.expect.output_matches {
            let filled = vars::interpolate_regex(pattern, &scope.vars)?;
            let re = regex::Regex::new(&filled)
                .map_err(|e| format!("case defect: `{pattern}` is not a valid regex ({e})"))?;
            if !re.is_match(&output) {
                return Err(format!(
                    "hook `{}` output does not match /{filled}/, so it did not show it did the \
                     thing — an exit code alone is a claim, not evidence\n      output: {}{because}",
                    h.hook,
                    head_lines(&output)
                ));
            }
        }
        for pattern in &h.expect.output_not_matches {
            let filled = vars::interpolate_regex(pattern, &scope.vars)?;
            let re = regex::Regex::new(&filled)
                .map_err(|e| format!("case defect: `{pattern}` is not a valid regex ({e})"))?;
            if re.is_match(&output) {
                return Err(format!(
                    "hook `{}` output matches /{filled}/ and must not\n      output: {}{because}",
                    h.hook,
                    head_lines(&output)
                ));
            }
        }
        if let Some(name) = &h.capture_stdout {
            scope.vars.insert(name.clone(), output.trim().to_string());
        }
        Ok(())
    }

    // ---------------------------------------------------------------- callbacks

    fn do_callback_receiver(&self, r: &CallbackReceiver, scope: &mut Scope) -> Result<(), String> {
        let bind = self
            .profile
            .callback
            .bind
            .as_deref()
            .unwrap_or("127.0.0.1:0");
        let receiver = Receiver::start(
            bind,
            self.profile.callback.advertise.as_deref(),
            r.respond_status,
        )?;
        scope
            .vars
            .insert(r.bind_url_to.clone(), receiver.url.clone());
        scope.receivers.push(receiver);
        Ok(())
    }

    fn do_callback_assert(&self, a: &CallbackAssert, scope: &Scope) -> Result<(), String> {
        let receiver = scope.receivers.last().ok_or_else(|| {
            "no callback receiver is running; an earlier step must start one".to_string()
        })?;
        let timeout = vars::parse_duration(&a.timeout)?;
        let arrived = receiver.wait_for(a.at_least.max(1), timeout);
        if arrived < a.at_least {
            let because = a
                .because
                .as_deref()
                .map(|w| format!("\n      because: {w}"))
                .unwrap_or_default();
            return Err(format!(
                "{arrived} callback(s) arrived within {}, expected at least {}{because}",
                a.timeout, a.at_least
            ));
        }
        let captured = receiver.captured();

        for check in &a.checks {
            crate::signing::check(*check, &captured, &self.profile.callback).map_err(|reason| {
                match &a.because {
                    Some(why) => format!("{reason}\n      because: {why}"),
                    None => reason,
                }
            })?;
        }
        Ok(())
    }

    // ---------------------------------------------------------------- fixtures

    fn do_for_each_fixture(
        &self,
        f: &ForEachFixture,
        case: &Case,
        scope: &mut Scope,
    ) -> Result<(), String> {
        let dir = case.requires.fixtures.as_deref().ok_or_else(|| {
            "for_each_fixture needs `requires.fixtures:` naming a directory".to_string()
        })?;
        let path = self.fixtures_dir(dir);
        let mut files: Vec<PathBuf> = std::fs::read_dir(&path)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "json"))
            .collect();
        files.sort();

        if files.len() < f.at_least {
            return Err(format!(
                "{} holds {} fixture(s), and the specification defines {} interaction patterns \
                 that must each be expressible (§5.6)",
                path.display(),
                files.len(),
                f.at_least
            ));
        }

        let mut excluded = Vec::new();
        for file in files {
            let raw = std::fs::read_to_string(&file)
                .map_err(|e| format!("cannot read {}: {e}", file.display()))?;
            let name = file
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            let doc: serde_json::Value = serde_json::from_str(&raw)
                .map_err(|e| format!("{} is not valid JSON: {e}", file.display()))?;

            if let Some(member) = &f.when_present {
                let present = expect::resolve(&doc, member)
                    .first()
                    .is_some_and(|hit| !hit.value.is_null());
                if !present {
                    excluded.push(name);
                    continue;
                }
            }

            let raise = match expect::resolve(&doc, "raise").first() {
                Some(hit) if !hit.value.is_null() => hit.value.clone(),
                _ => doc.clone(),
            };
            scope.vars.insert("fixture_name".to_string(), name.clone());
            scope
                .vars
                .insert("fixture_body".to_string(), format!("!json:{}", raw.trim()));
            scope
                .vars
                .insert("fixture_raise".to_string(), format!("!json:{raise}"));
            self.run_steps(&f.steps, case, scope).map_err(|fail| {
                format!("fixture `{name}`, step `{}`: {}", fail.step, fail.reason)
            })?;
        }

        if let Some(expected) = f.expect_excluded {
            if excluded.len() != expected {
                return Err(format!(
                    "{} fixture(s) were excluded by `when_present: {}` and the case expects \
                     exactly {expected}: {excluded:?}",
                    excluded.len(),
                    f.when_present.as_deref().unwrap_or("<none>")
                ));
            }
        }
        Ok(())
    }
}

fn check_all(
    doc: &serde_json::Value,
    matchers: &[Matcher],
    vars: &BTreeMap<String, String>,
) -> Result<(), String> {
    for matcher in matchers {
        expect::check(doc, matcher, vars)?;
    }
    Ok(())
}

fn text_of(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn first_line(text: &str) -> String {
    let line = text.lines().next().unwrap_or("").trim();
    let head: String = line.chars().take(200).collect();
    head
}

/// The first few non-empty lines of a hook's output.
///
/// `first_line` is enough when the complaint is an exit code. When the complaint is that the
/// evidence a case asked for is missing, one truncated line is not enough to see that it is — the
/// person reading the failure needs to see what the hook printed instead.
fn head_lines(text: &str) -> String {
    let lines: Vec<String> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .take(6)
        .map(|l| l.trim().chars().take(200).collect::<String>())
        .collect();
    if lines.is_empty() {
        return "(the hook printed nothing at all)".to_string();
    }
    lines.join("\n              ")
}

fn percent(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn walk_keys(
    value: &serde_json::Value,
    path: String,
    forbidden: &[String],
    allowed: &[String],
    found: &mut Vec<String>,
) {
    match value {
        serde_json::Value::Object(fields) => {
            for (k, v) in fields {
                let child = if path.is_empty() {
                    k.clone()
                } else {
                    format!("{path}.{k}")
                };
                if forbidden.contains(k) && !allowed.iter().any(|a| path_allows(a, &child)) {
                    found.push(child.clone());
                }
                walk_keys(v, child, forbidden, allowed, found);
            }
        }
        serde_json::Value::Array(items) => {
            for (i, v) in items.iter().enumerate() {
                walk_keys(v, format!("{path}[{i}]"), forbidden, allowed, found);
            }
        }
        _ => {}
    }
}

fn path_allows(pattern: &str, path: &str) -> bool {
    // `evidence[].kind` allows `prompt.evidence[3].kind`: `[]` matches any index, and the pattern
    // matches any suffix of the concrete path, so a case need not spell out every nesting.
    let normalized = regex::Regex::new(r"\[\d+\]")
        .map(|re| re.replace_all(path, "[]").to_string())
        .unwrap_or_else(|_| path.to_string());
    normalized == pattern || normalized.ends_with(&format!(".{pattern}"))
}

fn run_command(command: &str, args: &BTreeMap<String, String>) -> Result<(i32, String), String> {
    let mut cmd = std::process::Command::new("sh");
    cmd.arg("-c").arg(command);
    for (k, v) in args {
        cmd.env(format!("HANDOFF_ARG_{}", k.to_uppercase()), v);
    }
    let out = cmd
        .output()
        .map_err(|e| format!("cannot run `{command}`: {e}"))?;
    let mut text = String::from_utf8_lossy(&out.stdout).to_string();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    Ok((out.status.code().unwrap_or(-1), text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowed_paths_match_by_suffix_and_ignore_indices() {
        assert!(path_allows("evidence[].kind", "prompt.evidence[3].kind"));
        assert!(path_allows("target.kind", "via.target.kind"));
        assert!(!path_allows("target.kind", "kind"));
    }

    #[test]
    fn forbidden_keys_are_found_but_allowed_paths_are_not() {
        let doc = serde_json::json!({
            "kind": "approval",
            "prompt": {"evidence": [{"kind": "link"}]}
        });
        let mut found = Vec::new();
        walk_keys(
            &doc,
            String::new(),
            &["kind".to_string()],
            &["evidence[].kind".to_string()],
            &mut found,
        );
        assert_eq!(found, vec!["kind".to_string()]);
    }
}
