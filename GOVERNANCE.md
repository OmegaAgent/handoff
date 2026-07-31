# Governance

## Who decides

**Handoff is a BDFL project. The maintainer is the project owner, Noureddin Bakir.** Final say on the
spec, the core, and what ships rests there.

This is stated plainly rather than dressed up as a committee that does not exist. A one-person
project with an invented steering group is less trustworthy than a one-person project that says so.

**The counterweight offered in exchange:** every `spec/` change is public before it ships in any
managed service, with no private-first window. If a protocol behaviour appears in a hosted product
before it appears in a merged pull request here, that is a governance failure and you should file it
as one.

Maintainership will broaden when there are contributors with sustained review history. When that
happens it will be recorded here as a diff, not announced as a policy.

## The spec-change process

`spec/` is normative. Changing it is a different act from changing code, and it has a different bar.

1. **Open an issue** describing the problem the change solves, in terms of something an implementer
   or a responder can observe. "The spec is unclear about X" is a valid problem. "It would be nice
   if" is not, unless it comes with the case below.
2. **Write the rationale.** What breaks today. What the alternatives were. Why this one.
3. **Bring one of two things:**
   - **two independent implementations** of the proposed behaviour, or
   - **a conformance case that fails before the change and passes after it.**

   The second is usually cheaper and is the one we expect.
4. **Merge lands spec text and conformance case together.** A `spec/` change that does not touch
   `conformance/` is blocked by CI. This is deliberate: a specification whose executable half is
   optional stops being a specification within about two releases.

**A change that cannot be expressed as a conformance case is not a protocol change.** It may still
be a good change — to the reference server, to an adapter, to the docs — but it does not go in
`spec/`.

## Version policy

**The spec and the implementations version independently, by tag namespace.** A single repository
version number would force a spec bump every time a crate patch ships, which is how a spec version
stops meaning anything.

| Tag namespace | Covers | Example |
|---|---|---|
| `spec/vX.Y` | the normative protocol: `spec/**` | `spec/v0.1` |
| `core/vX.Y.Z` | the Rust workspace under `core/` (all crates move together) | `core/v0.3.1` |
| `sdk-ts/vX.Y.Z` | `sdk/ts` and `sdk/types` | `sdk-ts/v0.2.0` |
| `sdk-py/vX.Y.Z` | `sdk/python` (`handoff-human` on PyPI) | `sdk-py/v0.2.0` |
| `conformance/vX.Y` | `conformance/**` and the runner's case expectations | `conformance/v0.1` |

### Protocol versioning

- The wire version is carried in the request (`protocol: "handoff/0.1"`) and is reported by
  `GET /v1/meta`, whose `core_version` field is the running core version.
- The **major** version appears in the path (`/v1`). A new major ships alongside the old one; the
  old one runs for at least 12 months past the successor's general availability.
- **MINOR** = additive optional request fields, new response fields, new channel capabilities, new
  optional endpoints, new enum members in *response-only* positions. Clients MUST ignore unknown
  fields.
- **MAJOR** = any change to a state name, to the set of terminal states, to receipt field semantics,
  or to the signature scheme.
- Servers accept version *N* and *N−1*. SDKs send the lowest version that carries every field they
  need.
- **Versioned envelopes fail closed.** A server that does not understand a `requires` version
  rejects the request and **creates nothing**. Never partial acceptance.
- **Extension namespace:** keys prefixed `x-` are stored and returned verbatim and are never
  interpreted. Vendors extend there rather than squatting on core names.

### Crate versioning

All crates in `core/` share one version and are released together under a single `core/vX.Y.Z` tag.
Independent crate versions across six crates that only ever ship as a set is bookkeeping with no
reader.

### What "released" means

**A release is a `spec/` tag plus a passing conformance run against both the reference server and
any managed service claiming that version.** Tying the definition of "released" to the managed
service passing the open suite is what stops the open core becoming a stale marketing artifact. If
the managed service cannot pass the open suite, the release is not a release and the deploy is red.

The check that tells you whether this is working: `GET /v1/meta` on any managed deployment returns
the core version and the spec version it is running. If that drifts more than one minor release
behind the latest published tag, the governance model is failing and it is visible to everyone,
including the maintainer.

## The open/closed boundary

Some parts of the product around Handoff are commercial and closed. The line is drawn by a test, not
by convenience, and the test is public:

> Open everything required to define, run, and independently verify a handoff on a single tenant's
> own machine. Keep closed only what derives its value from a third party operating it: shared
> infrastructure, cross-tenant knowledge, and promises that outlive the customer's own process.

Three clauses keep that honest, and they bind the maintainer:

- **Anti-crippleware.** Everything a managed service needs to be *correct* is open; only what it
  needs to be *well operated* is closed. A disabled feature flag in the open core is a violation.
- **Verification.** A capability is never closed by withholding a data format. Receipts may be
  signed by a pluggable signer, but the **verifier is open, always**. A receipt only the vendor can
  verify is a vendor claim, not evidence.
- **Exit.** Export is open. Anyone must be able to leave with their requests, receipts, and audit
  trail using only open code.

If you believe a change violates one of these, say so in the pull request and cite the clause. That
argument takes precedence over a maintainer preference, and the maintainer has to answer it in
public.

## Trademarks

Trademark conditions live in `TRADEMARKS.md` and never in the licence text. The licence grants
copyright and patent rights; it grants no trademark rights, and modifying OSI licence text to add
conditions would make the licence non-standard and unreviewable.
