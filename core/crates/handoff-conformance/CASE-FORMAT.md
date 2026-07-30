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
  hook: storage_update_receipt
  args: {receipt_id: "${receipt}"}
  expect:
    exit_code_not: 0            # or exit_code: 0
    output_matches: ["refused"]
    because: "..."
  capture_stdout: some_var
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
keys), `${case_id}`, `${base_url}`.

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
