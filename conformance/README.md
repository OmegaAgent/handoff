# Conformance suite

Declarative cases that decide whether an implementation is Handoff-compatible. The runner is
[`core/crates/handoff-conformance`](../core/crates/handoff-conformance); it takes a base URL and does
not care what is listening there.

```
handoff-conformance --base-url http://127.0.0.1:8080
```

**Status: no cases yet.** They land in milestone H1, where every one of them is expected to fail
against a stub server, and go green in H2 against the reference implementation. The runner exits
non-zero while the case count is zero, so no pipeline can report conformance it has not measured.

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
The format is defined alongside the first cases in H1. Until then, the most useful contribution is an
**issue describing an edge you hit in production** and what the correct behaviour should be. Edges
found in the field are worth more than edges imagined at a desk, and the four the design already
calls hardest are: double-resolve conflict, attempt lapse, callback retry, and receipt verification.
