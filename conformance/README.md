# Conformance suite

Declarative cases that decide whether an implementation is Handoff-compatible. The runner is
[`core/crates/handoff-conformance`](../core/crates/handoff-conformance); it takes a base URL and does
not care what is listening there.

```
handoff-conformance --base-url https://your-deployment.example.com/v1 --profile your-profile.yaml
```

**Status: 25 Level 1 cases and 1 Level 2 case. The reference server passes all of them.** They were written before any conforming
server existed, and the last thing done to them was to run them against a server that answers `501`
to everything and confirm the report reads `0/25 passing` with twenty-five individually named
failures and a non-zero exit. A suite that cannot fail loudly is not a suite. Against the reference
implementation they are green; `conformance/GATE.md` records both directions.

The runner checks the case set against `spec/conformance-map.json` before running anything, so a
case §18 defines with no file, or a file §18 does not define, stops the run rather than quietly
changing the total.

Two things a deployment must supply beyond a base URL, both documented in
`core/crates/handoff-conformance/profile.example.yaml`: credentials for the principal aliases the
cases name, and the handful of commands that reach below the HTTP API — C-15 must attempt a receipt
mutation at the **storage** layer, because §9.4 puts the application inside the threat model, and
C-7 must grep the logs. A case whose requirements are missing **fails**; it is never skipped.

The case format is documented in `core/crates/handoff-conformance/CASE-FORMAT.md`.

## Why this directory is not a test folder

This is the project's governance instrument, and three rules in
[`GOVERNANCE.md`](../GOVERNANCE.md) hang off it:

- **A `spec/` change must arrive with a case here.** CI blocks a pull request that edits `spec/` and
  nothing under `conformance/`. A specification whose executable half is optional stops being a
  specification within about two releases.
- **A core behaviour change without a case here is not merged.** This is what stops the suite
  lagging the implementation, which is the normal way a conformance suite dies.
- **The suite gates deploys, including the maintainer's own hosted service.** A service that cannot
  pass the open suite has a red build. That turns "we did not quietly fork the core" into a check
  anyone can rerun.

## Claiming conformance

"Handoff-compatible" is claimable for a specific protocol version only against a **published passing
run** for that version: the case list, the result per case, the suite version, and the date, posted
somewhere a skeptic can find them and rerun them against your endpoint. See
[`TRADEMARKS.md`](../TRADEMARKS.md).

## Contributing a case

Cases are declarative YAML under `cases/`, so a case is readable by someone who does not write Rust.
The format is `core/crates/handoff-conformance/CASE-FORMAT.md`; every case-specific fact lives in the
YAML, and the runner is an interpreter that knows nothing about any individual case.

The most useful contribution is an **issue describing an edge you hit in production** and what the
correct behaviour should be. Edges found in the field are worth more than edges imagined at a desk,
and the four the design already calls hardest are: double-resolve conflict, attempt lapse, callback
retry, and receipt verification.
