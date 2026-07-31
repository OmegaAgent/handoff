# Contributing to Handoff

Handoff is a protocol before it is an implementation. That shapes what we want from a
contribution: a change to how the protocol *behaves* has to be arguable against the spec, and a
change to what the reference implementation *does* has to be provable by a conformance case.

## Developer Certificate of Origin — sign off, no CLA

**Every commit must carry a `Signed-off-by:` trailer. There is no CLA and there will not be one.**

We use the [Developer Certificate of Origin 1.1](https://developercertificate.org/). Signing off
means you assert the DCO's four clauses about the code you are contributing — essentially, that you
wrote it or have the right to submit it under the project's licence.

Add the trailer automatically:

```
git commit -s -m "your message"
```

which appends:

```
Signed-off-by: Your Name <your.email@example.com>
```

The name and email must be real and must match your `user.name` / `user.email`. If you forgot on
the last commit:

```
git commit --amend -s --no-edit
```

For a whole branch:

```
git rebase --signoff main
```

CI enforces this. A pull request with an unsigned commit will not merge.

### Why a DCO and not a CLA

Apache-2.0 §5 already licenses your contribution to the project under the project's own licence,
including the §3 patent grant. The substantive thing a CLA buys is therefore already covered. What
a CLA additionally buys is the option to **relicense the project later** — that is, the option to
close it. We are not asking for that option, because asking for it would contradict the reason this
repository exists.

The cost is explicit and we accept it: with a DCO, the maintainers cannot unilaterally relicense the
open core. That is the point.

## Licensing of contributions

- Contributions to `spec/`, `core/`, `conformance/`, `ui/`, and `docs/` are licensed **Apache-2.0**.
- Contributions to `sdk/**` are licensed **MIT** (see the per-directory `LICENSE` files).

By signing off, you contribute under the licence that already governs the directory you are editing.
Do not introduce a file under a third licence without opening an issue first.

## What we especially want

In rough order of usefulness:

1. **Delivery adapters** — a new channel behind the `DeliveryChannel` port. A channel declares its
   capabilities; it must not require a branch anywhere in the core.
2. **SDK ports to new languages** — Go, Ruby, Java, whatever you actually use. The wire contract is
   `spec/openapi.yaml` and the fixtures are normative; an SDK that invents its own encoding is a bug.
3. **Conformance cases** — especially for edges you hit in production that the suite does not cover.
4. **Documentation**, including corrections to the spec's prose where it is ambiguous. An ambiguity
   in a normative document is a real defect, not a nitpick.

## Rules that gate a merge

These are not style preferences. They are what keeps the spec and the implementation from drifting
apart, which is the normal way a protocol project dies.

- **A `spec/` change requires** an issue, a written rationale, and **either** two independent
  implementations **or** a conformance case that fails before the change and passes after.
- **A `handoff-core` behaviour change that is not covered by a conformance case is not merged.**
  If the behaviour cannot be expressed as a conformance case, that is evidence the behaviour is not
  actually part of the protocol — say so in the PR and we will discuss where it belongs.
- **Show the check failing.** A new guard, matcher, or conformance case must arrive with evidence of
  it going red — break the thing it protects, paste the failure, restore. A check nobody has watched
  fail has not been tested; it has tested the code, with an instrument of unknown sensitivity. Six
  checks in this repository passed for months while measuring nothing, and every one was written by
  somebody trying to prevent exactly what it missed. `conformance/README.md` names the three shapes
  that recur.
- **No `[patch]`, no path dependency, no fork of a `handoff-*` crate** in any consumer of this
  repository, including our own managed service. If the managed service needs a change to the core,
  the change lands here first and the managed pin is bumped. CI greps for this. A
  `[patch.crates-io]` entry pointing at a `handoff-*` crate is the moment a fork begins.
- **No disabled feature flags, no dormant paywall, no capability that exists only to be switched
  off.** Everything the managed service needs to be *correct* lives here. Only what it needs to be
  *well operated* lives elsewhere. A gate in the open core is a boundary violation and an invitation
  to the first hostile fork.
- **Unknown fields are ignored, never rejected**, except where the spec says a version envelope
  fails closed. Extension keys are prefixed `x-` and are stored and returned verbatim.

## Working on the code

```
core/           Rust workspace. cargo fmt, cargo clippy -- -D warnings, cargo test.
sdk/python/     Python 3.9+, standard library only. Keep it that way; it is a product property.
sdk/ts/         TypeScript. Zero runtime dependencies.
conformance/    Declarative YAML cases. 25 Level 1, plus C-17 at Level 2.
spec/           Normative. Read GOVERNANCE.md before proposing a change.
```

Before opening a PR:

```
cd core && cargo fmt --all -- --check && cargo clippy --all-targets -- -D warnings && cargo test
```

CI runs the same commands and requires no secrets. If a change to CI would require a secret, that is
a design problem with the change, not with CI.

## Pull requests

- One logical change per PR. A refactor and a behaviour change in one diff cannot be reviewed.
- Say what breaks. If nothing breaks, say that too, and say how you know.
- Do not include generated artifacts, screenshots containing tokens or request identifiers, or any
  `.env`. See `SECURITY.md`.
- Claims in a PR description should be checkable. "Verified by running X, output Y" beats "should
  work". This repository inherits the honesty discipline recorded in `DISCLOSURE.md`: state what was
  measured, state what was not.

## Reporting a security issue

Do not open a public issue. See `SECURITY.md`.

## Code of conduct

Be straightforward and stay on the technical substance. Disagreement about a design is the point of
a design review; contempt for the person on the other side of it is not. The maintainer will remove
anyone who cannot tell the difference.
