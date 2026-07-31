# Handoff

**An open protocol for the moment a program needs a person.**

Agents run continuously. The people who can unblock them do not, and those people are still the ones
accountable for the decision. Handoff specifies what happens in between: how a request for a human
decision is stated, how it reaches someone, how their answer comes back, and what record survives
afterwards.

The claim the whole design is built to support is deliberately narrow:

> **One human answer, delivered to your agent exactly once, as typed data, authorizing exactly one
> effect.**

Every clause of that is load-bearing:

- **One human answer.** A specific person acted, and the record says who. Clearance is *asserted* by
  a human, never *inferred* from a side effect. That distinction is the reason this project exists;
  the section on prior art below explains where it came from.
- **Exactly once.** An answer is consumed when it is used. A stored yes cannot be spent a second
  time by a retry, a replay, or a duplicate delivery.
- **As typed data.** The answer is a value against a declared schema, not free text that a prompt has
  to interpret. A field a renderer cannot draw is a field a human cannot answer, so unknown field
  types are rejected rather than degraded into a text box.
- **Authorizing exactly one effect.** The answer is bound to the specific thing it was shown
  against. It does not generalize to the next call, the next run, or a similar-looking request.

### What Handoff does not claim

It does not resume your program's execution. There is no stack snapshot, no continuation capture, no
reaching into your runtime. Your code asks, waits however it prefers (a blocking call, a webhook, a
poll), and receives an answer. What it does with that answer is entirely yours. Any description that
implies otherwise is overselling, including any we might write later.

It also does not make a self-hosted deployment good at reaching people. Delivery through email, SMS,
or voice depends on sender reputation, aligned DNS records, a reviewed app, and carrier
relationships. None of that ships in a container, and this repository will not pretend it does.

## Status

**Pre-release. The protocol runs; it has not been published, and there is no hosted service running
this reference implementation.** (`examples/night-hack/` is preserved prior art, not this
implementation, and `SECURITY.md` says what is known and not known about where it has been
deployed.)

The reference server passes all 25 Level 1 conformance cases. It does not pass C-17, the
optional Level 2 continuation case, and deliberately cannot — see below. Read `docs/hostile-review.md` before
relying on any of this: an independent review pass found defects that are open at the time of
writing, and `BACKLOG.md` states the gaps plainly rather than leaving them to be discovered.

| Component | State |
|---|---|
| `spec/` — the normative protocol | v0.1, frozen. 21 invariants, OpenAPI 3.1, signing test vectors that reproduce. |
| `conformance/` — the suite | 25 Level 1 cases + 1 Level 2. Written before the server, and demonstrably red against one that implements nothing. |
| `core/` — the Rust reference implementation | `handoffd` passes **25/25 Level 1**. Level 2 (`continuation`) is not implemented and is not advertised. |
| `sdk/python` — `handoff-human` | 0.2.0. Standard library only. |
| `sdk/ts` — `@handoffproto/sdk` | Zero runtime dependencies. |
| `managed/` — the hosted adapter | Closed-source, unpublished. Refuses where a dependency does not exist rather than pretending. |
| `ui/responder` — a standalone human-facing page | **Not built.** It appears in the layout below as intended; it is absent on disk. |

Nothing in this tree has been published. The one exception predates it: `handoff-human` 0.1.0 is
already on PyPI from the hackathon build, under the MIT grant `NOTICE` relies on. The 0.2.0 in
`sdk/python` is not published, and the crate and npm names are not yet reserved.

This table is the honest state as of the last commit. If it disagrees with something else in this
repository, this table is what was checked — and the disagreement is a defect worth reporting,
because a status document that is trusted and stale is worse than one nobody reads.

## Where the spec lives

`spec/` is normative and is the source of truth for every implementation, including ours:

| File | What it is |
|---|---|
| `spec/handoff-protocol-v0.1.md` | The normative protocol, using RFC 2119 keywords |
| `spec/openapi.yaml` | The wire contract, and the source the typed SDKs are generated from |
| `spec/schemas/*.schema.json` | Request, receipt, policy, and delivery-attempt schemas |
| `spec/signing.md` | The receipt and callback signature scheme, with test vectors |
| `spec/CHANGELOG.md` | What changed and when |

The spec is versioned independently of the implementations, by tag namespace (`spec/v0.1`,
`core/v0.3.1`, `sdk-py/v0.2.0`, and so on). See [GOVERNANCE.md](GOVERNANCE.md) for the version policy
and for the process a spec change has to go through, which is stricter than the one for code.

## Repository layout

```
spec/               The normative protocol. Apache-2.0.
core/               Rust workspace. Apache-2.0. Intended for crates.io; not yet published.
  crates/handoff-protocol/        types, state machine, policy evaluation. No I/O.
  crates/handoff-core/            the engine and the port traits a deployment implements
  crates/handoff-store-postgres/  reference store and its own migration set
  crates/handoff-adapters/        delivery channels, feature-gated per channel
  crates/handoff-server/          the reference server. Binary: handoffd
  crates/handoff-conformance/     the suite runner. Takes any base URL.
conformance/        Declarative cases. The governance instrument, not a test helper.
sdk/python/         handoff-human. MIT.
sdk/ts/             @handoffproto/sdk. MIT. Zero runtime dependencies.
ui/responder/       PLANNED, not present. A standalone page for the person answering. Apache-2.0.
examples/           Worked examples, including the original hackathon build.
docs/               Documentation source.
```

## Self-hosting

Self-hosting is the point rather than a concession, so it is worth being precise about what it does
and does not get you.

**The shape today:** point `handoffd` at a Postgres database, give it a configuration file naming the
people it can reach and the channels it may use, and run it. It has
no dependency on any hosted service, no phone-home, no licence key, and no disabled feature waiting
to be switched on. `CONTRIBUTING.md` makes a dormant gate in this repository a mergeable-blocking
defect rather than a matter of taste.

**What a self-hosted deployment genuinely guarantees:** correctness of the state machine;
exactly-one-effect semantics; waits that survive a process restart; a complete local audit trail;
full data export; and no runtime dependency on anyone else being alive, solvent, or interested.

**What it structurally cannot guarantee**, stated here rather than discovered later:

- **Independent attestation.** A receipt signed with a key you control is adequate for internal
  control and worthless as evidence *against* you. Only a party who is not the operator can attest to
  what an operator's system recorded. That is the definition of a third party, not a feature we are
  withholding.
- **Deliverability.** See above. A fresh deployment starts at zero reputation on every channel.
- **Cross-tenant signals.** "This number has been paged 400 times today by twelve different senders"
  is not computable by one deployment that cannot see the others.
- **Retention as a promise.** Your audit trail is exactly as durable as your backups. A promise needs
  a promisor.

Meanwhile you can read the spec and implement it yourself. That is a supported outcome, and the
conformance suite exists so you can prove you got it right without asking us.

## The open and closed line

Some of the product around Handoff is commercial. The boundary is drawn by a published test rather
than by convenience:

> Open everything required to define, run, and independently verify a handoff on a single tenant's
> own machine. Keep closed only what derives its value from a third party operating it: shared
> infrastructure, cross-tenant knowledge, and promises that outlive the customer's own process.

In practice almost everything is open. The closed surface reduces to a delivery fleet, the
cross-tenant view, third-party attestation, and one company's own billing and organization model,
none of which is protocol. Three clauses bind the maintainer and are written into
[GOVERNANCE.md](GOVERNANCE.md): no crippleware, the receipt verifier is open unconditionally, and
export is open. If you think a change violates one, cite the clause in the pull request; it has to be
answered in public.

The mechanism that keeps this from becoming a slogan is the conformance suite. A hosted service that
cannot pass the open suite has a red build, which turns "we did not quietly fork the core" from an
intention into a check anyone can rerun.

## Licensing

| Subtree | Licence |
|---|---|
| `spec/`, `core/`, `conformance/`, `ui/`, `docs/`, repository default | **Apache-2.0** ([LICENSE](LICENSE)) |
| `sdk/**` | **MIT** ([LICENSE-MIT](LICENSE-MIT), plus per-directory `LICENSE` files) |
| `examples/night-hack/` | **MIT**, unchanged from its original release |

Apache-2.0 on the spec and the implementation because its §3 grants patent rights expressly and
terminates them on patent litigation against the project. MIT on the SDKs because a thin client gets
vendored into everything, including projects whose licences make Apache-2.0 awkward, and its patent
exposure is nil.

The Python SDK was first published under MIT, `Copyright (c) 2026 Noureddin Bakir`. That grant stands
and is not rewritten here. Trademark conditions live in [TRADEMARKS.md](TRADEMARKS.md) and never in
the licence text.

## Contributing

Read [CONTRIBUTING.md](CONTRIBUTING.md) first. The short version:

- **Sign off your commits** (`git commit -s`). We use the Developer Certificate of Origin. **There is
  no CLA and there will not be one**, because the only thing a CLA would buy beyond what Apache-2.0
  §5 already grants is the option to close this later.
- **A `spec/` change needs a conformance case** that fails before it and passes after, or two
  independent implementations.
- **A core behaviour change without a conformance case is not merged.**

Most wanted: delivery adapters, SDK ports to other languages, conformance cases, and corrections to
ambiguous spec prose. An ambiguity in a normative document is a real defect.

Security issues go to the private channel in [SECURITY.md](SECURITY.md), never a public issue. Note
that in this protocol a request identifier can itself be a capability, so treat one like a
credential.

## Prior art, and the honesty discipline

Handoff started as a four-hour build at founders.inc Night Hack on 2026-07-24. That build is
preserved under [`examples/night-hack/`](examples/night-hack/) with a README describing what it is
and, more usefully, what it is not.

It is kept because it is the evidence. It also demonstrated the failure the protocol exists to
prevent: before it, "the human is finished" was inferred from a page URL or title changing, which
silently misses a wall cleared in place. A person ticks a verification checkbox, nothing about the
page identity changes, and the agent waits until its deadline for an obstacle that is already gone.
The fix generalizes into the rule stated at the top of this file: a human action is recorded, never
detected.

[DISCLOSURE.md](DISCLOSURE.md) is that build's honest accounting of what existed beforehand, what was
made during the event, and what was measured rather than asserted. It is kept at the root of this
repository on purpose. The standard it sets applies to everything here: say what was checked, say how
it was checked, and say plainly what was not.
