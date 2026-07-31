# The conformance case format

A case file is the normative, human-readable statement of one conformance test. This document is
what a non-Rust implementer needs in order to read a case, argue with it, or contribute one.

Nothing case-specific lives in the runner. `handoff-conformance` is an interpreter for the format
below and holds no knowledge of any individual case, so the suite can be audited by reading
`conformance/cases/` rather than by reading Rust.

## The shape of a file

```yaml
id: C-3                    # stable, exactly as in specification §18. Never renumbered.
title: Two simultaneous answers settle a request exactly once
level: 1                   # 1 = required of every Server. 2 = the optional §14 extension.
invariants: [I5]           # from §17, and must match spec/conformance-map.json exactly
spec:                      # where this comes from, for a reader tracing it back
  - "§6.2 R5 — the answer is a conditional write on state = 'pending'"
rationale: >
  Why the case exists: the failure it is designed to catch, in prose. This is the part a
  reviewer reads to decide whether the assertions below are the right ones.

requires:                  # what the deployment must supply for the case to run at all
  principals: [machine_a, human_editor]
  hooks: [logs]
  fixtures: use-cases

steps:
  - name: raise a request  # what this step does, in the words of the specification
    http: { ... }          # exactly one action key per step
```

The runner checks every file against `spec/conformance-map.json` before running anything. A case
§18 defines with no file, a file §18 does not define, a level that disagrees, or an invariant claim
the map does not support all stop the run.

## Nothing is ever skipped

A case whose `requires` the deployment has not supplied is a **failure** with a stated reason, never
a skip. A suite that silently skips what it cannot run reports conformance it did not measure, which
is worse than no suite at all.

## Actions

Each step carries exactly one of these.

### `http` — one call

```yaml
http:
  method: POST
  path: /requests/${req}/answer     # below the base URL; ${var} interpolated
  as: human_editor                  # principal alias; `anonymous` sends no credential
  headers: {Idempotency-Key: "c03-${run_id}"}
  query: {waiter_ref: "run:x"}
  body: {values: {decision: approve}}
  expect:
    status: 200                     # or status_in: [200, 204]
    body: [ <matchers> ]
    headers: [ <matchers> ]
  capture: {req: id}                # bind a response value to a variable
  capture_headers: {rl: X-RateLimit-Limit}
```

### `concurrent` — calls released at the same instant

Every call is prepared fully, then all are released off a barrier from separate threads, so what
contends is the Server's settling write rather than the suite's own string formatting. `count` is
how many responses must fall into each class; every response must be claimed by exactly one class.

```yaml
concurrent:
  calls: [ <http calls, without expect/capture> ]
  expect_outcomes:
    - {count: 1, status: 200, body: [...], because: "..."}
    - {count: 1, status: 409, body: [...], because: "..."}
```

### `poll` — repeat until it holds

For sweeps the Server performs on its own clock.

```yaml
poll:
  call: <an http call>
  timeout: PT30S
  interval: PT2S
```

### `abandon_poll` — a client process dying mid-wait

Opens the connection, holds it, and drops it without reading the response.

```yaml
abandon_poll:
  path: /waiters/run%3Ax/signals
  as: machine_a
  query: {wait: "30"}
  abandon_after: PT1S
```

### `advance_clock` — move time forward

Uses the deployment's `advance_clock` hook when it has one. Without the hook it sleeps for real, and
fails when the duration exceeds the profile's `max_real_sleep`.

```yaml
advance_clock: {by: PT5S}
```

### `scan` — search every artifact

C-7 and C-8 are scans, not unit tests. Sources: `traffic` (every request and response body recorded
in this case), `urls`, `headers`, `request`, `receipt`, `deliveries`, `signals`, `logs`. A source
that cannot be read is a failure, not an exemption.

```yaml
scan:
  for: ["${secret_value}"]
  in: [traffic, urls, headers, request, receipt, deliveries, signals, logs]
  request: "${req}"
  waiter_ref: "run:x"
  as: machine_a
  because: "..."
```

### `forbid_keys` — no key by this name, anywhere

Walks every JSON body recorded in this case. `allow_paths` matches by suffix and treats `[]` as any
index, so `evidence[].kind` permits `prompt.evidence[3].kind`.

```yaml
forbid_keys:
  keys: [kind]
  allow_paths: ["evidence[].kind", "target.kind"]
  because: "..."
```

### `hook` — a deployment-supplied command

The runner speaks only HTTP. Anything below the API is a command the deployment provides. It runs
with `sh -c`; arguments arrive as `HANDOFF_ARG_<UPPERCASE_KEY>`.

```yaml
hook:
  hook: channel_inbound
  args: {request_id: "${req}", channel: email}
  expect:
    exit_code: 0                # or exit_code_not: 0
    output_matches: ["channel_message=\\S+"]
    because: "..."
  capture_stdout: some_var
```

**A hook's output is the weakest evidence in this format, and the number of hooks is meant to go
down.** Two hostile reviews defeated hook-based assertions in a row: the first stubbed every hook
with `true` and `false`, the second printed the exact evidence the cases required, because every
value a case can demand is either handed to the hook as an argument or small enough to guess — and
the claimant writes the hook. Before adding a case that needs a hook, check whether the claim can be
computed from what the suite already read over HTTP (`verify_chain`) or decided by an observation the
suite makes afterwards (`storage_mutation`). Those are not conveniences; they are the two shapes that
survive a hostile profile.

### `verify_chain` — the suite recomputes the receipt chain

Reads the receipt listing over HTTP and walks it in the runner's own implementation of
`signing.md` §2.2 — canonical bytes, core hash, chain digest — which shares no code with any server
and is checked against the published vectors in `spec/fixtures/signing/`. The exported head must be
the head that walk arrives at. Then it rewrites each receipt in turn and requires the walk to break,
so a green walk is never a walk over nothing.

```yaml
verify_chain:
  as: machine_a
  receipts: /receipts               # the listing to walk
  head: /receipts/chain-head        # the exported head it must agree with
  must_include: ["${receipt}"]      # ids that must be in the walk
  standalone: ["${receipt}"]        # ids that must also verify alone, from their own prev_digest
  at_least: 1
  because: "..."
```

### `storage_mutation` — a mutation below the API, judged above it

Attempts a mutation through a deployment hook and decides the step on what HTTP shows afterwards.
The suite reads the object before and after; `refused` requires the bytes to be identical, `applied`
requires them to differ and the `observe.body` matchers to hold.

`applied` is how a case gets a **positive control** for a refusal: "the engine refused my write" and
"I never wrote" are the same observation from outside, so C-15 first aims the same command at a row
the engine permits and requires the value to appear over HTTP.

```yaml
storage_mutation:
  hook: storage_mutate
  args: {target: receipt, operation: update, id: "${receipt}", value: "${nonce}"}
  expect: refused                   # refused | applied
  output_matches: ["(?i)append.?only"]
  observe:
    path: /requests/${req}/receipt
    as: machine_a
    body:
      - path: chain.digest
        same_as: digest
  because: "..."
```

### `callback_receiver` / `callback_assert` — C-18

`callback_receiver` binds a local listener and exposes its URL. `callback_assert` runs named checks
over what arrived: `signed`, `sequence_monotonic_per_waiter`, `signature_verifies`,
`one_byte_tamper_rejected`, `replay_onto_other_delivery_rejected`, `stale_timestamp_rejected`,
`rotation_overlap_both_verify`, `redelivers_until_acked`, `carries_no_resolvable_url`,
`tenancy_not_derivable_from_body`. The signature checks implement `spec/signing.md` §1 and are
themselves checked against its published test vectors in the runner's unit tests.

```yaml
callback_receiver: {bind_url_to: callback_url, respond_status: 200}
```
```yaml
callback_assert:
  timeout: PT30S
  at_least: 2
  checks: [signed, redelivers_until_acked]
  because: "..."
```

### `for_each_fixture` — once per published fixture

Binds `${fixture_name}`, `${fixture_body}` (the whole document), and `${fixture_raise}` (its `raise`
member). `when_present` skips fixtures whose named member is null, and `expect_excluded` asserts how
many were skipped, so an exclusion is a claim the case makes rather than a filter that could quietly
grow.

```yaml
for_each_fixture:
  at_least: 8
  when_present: raise
  expect_excluded: 2
  steps: [ ... ]
```

## Matchers

A matcher is a path, one operator, and a `because` explaining what the specification requires. The
`because` is printed on failure, so a red case explains itself without the reader opening the spec.

```yaml
- path: error.code
  equals: already_answered
  because: "§6.7.2 — the Server returns a specific 409 carrying the settling record"
```

**Paths** are dotted with optional `[n]` indices and a `[]` wildcard: `error.code`, `data[0].id`,
`data[].org_id`. An empty path addresses the whole document.

**Operators**, the complete set:

| Operator | Holds when |
|---|---|
| `equals` / `not_equals` | the value does / does not equal the given JSON |
| `exists: true\|false` | the path resolves / does not resolve |
| `is_null: true\|false` | the value is / is not JSON null — distinct from absent |
| `matches` | the value is a string matching the regular expression |
| `length` / `length_at_least` | an array, string, or object of that size |
| `one_of` | the value is one of the listed values |
| `same_as` / `differs_from` | the value equals / differs from a captured variable |
| `all_equal` / `none_equal` | every / no value from a wildcard path equals the given value |
| `set_equals` | the wildcard path yields exactly this set — **length and identity** |
| `contains_text` / `not_contains_text` | the serialized value does / does not contain the substring |

Values carried by an operator are `${var}`-interpolated, so `none_equal: "${signal_id}"` means the
id an earlier step captured.

### Every operator, against a path that resolves to nothing

An assertion over an empty match set is the quietest way for a case to measure nothing: a path that
resolves to no values and a path that does not resolve at all are the same thing from the operator's
side, and both satisfy anything phrased as an absence. Three of the four set-shaped operators had a
guard for this and the fourth did not, which is a failure of the audit rather than of the operator —
so the audit is a test now
(`expect.rs`, `every_operator_is_audited_against_a_path_that_resolves_to_nothing`), and adding an
operator will not compile until its row here is true of it.

| Operator | Path matched nothing | Path matched many |
|---|---|---|
| `equals`, `not_equals`, `is_null`, `matches`, `length`, `length_at_least`, `one_of`, `same_as`, `differs_from`, `contains_text`, `not_contains_text` | **fails** — "`path` is absent" | **fails** — these compare one value; use a set operator or name the index |
| `exists: true` | **fails** | passes if any hit exists |
| `exists: false` | **passes**, and only this one does — but its container must resolve, or the absence is a fact about the case file rather than about the Server | n/a |
| `all_equal`, `none_equal` | **fails** — "matched nothing, so this proves nothing" | the assertion, applied to every hit |
| `set_equals` | **fails** — to assert a collection is empty, assert the collection's own `length: 0` | the assertion, over the whole set |

A body that is not JSON fails any step asserting on members, for the same reason: every path
resolves to nothing against an unparseable body, so a gateway error page would satisfy a case built
out of negatives.

### Use `set_equals`, not "contains", for anything tenant-scoped

§18 is explicit about this and the reason is worth repeating: a query missing its tenant predicate
returns a **superset**, and a containment assertion passes against a superset. `set_equals` compares
length and identity together, and is the only form that fails when a row that should have been
invisible is present.

## Variables

`${name}` is substituted in paths, header values, and strings inside a body. An unbound name is an
error, never an empty string — substituting nothing silently is how a test starts asserting against
`/requests//answer` and passing for the wrong reason.

Bound automatically: `${run_id}` (unique per invocation, so reruns do not collide on idempotency
keys), `${case_id}`, `${base_url}`, and `${nonce}` — a value no deployment can have seen before,
which is what makes "this value appeared in the store" an observation about *this* attempt rather
than about a row an earlier run left behind.

A string of the form `!json:<text>` is parsed as JSON rather than kept as a string, which is how a
whole fixture body becomes a request body.

## `when` — a deployment-choice guard

Where the specification permits a deployment to choose, a step can be guarded on a profile flag.
Both branches stay in the file so a reader sees both.

```yaml
- name: a deployment that forbids link_only refuses the answer
  when: {flag: link_only_permitted, is: false}
  http: { ... }
```

## Contributing a case

Open an issue describing an edge you hit in production and what the correct behaviour should be.
Edges found in the field are worth more than edges imagined at a desk. A case that lands must name
the invariant it proves, and `spec/conformance-map.json` must agree — the runner refuses to run
otherwise.
