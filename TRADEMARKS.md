# Trademark policy

**This document is not part of any licence.** The Apache License 2.0 (`LICENSE`) and the MIT License
(`LICENSE-MIT`) grant copyright and patent rights and grant **no** trademark rights — Apache-2.0 says
so expressly in §6. OSI-approved licence text must never be modified, so every trademark condition
lives here instead, where it can be read, argued with, and revised without producing a non-standard
licence that no legal review will pass.

Nothing here restricts what the licences allow you to do with the code. It governs only what you may
call the result.

## The marks

| Mark | Status | Notes |
|---|---|---|
| **Ωmegas**, and the **Ω** logo | Claimed, distinctive, defended | The genuinely distinctive assets. |
| **Handoff-compatible** | Conformance mark, conditioned (see below) | The enforceable lever. |
| "Handoff" as a bare word | **Weak, and we say so** | "Handoff" is descriptive for software. We do not claim exclusivity over the word and no part of this project's strategy depends on owning it. If you are worried about using the word "handoff" in ordinary English, don't be. |

Being straight about the third row matters more than it costs us. A trademark policy that overclaims
gets ignored in full, including the part that is real.

## Granted: nominative use

You may use the project's names truthfully, without permission and without a licence, to describe
what your software does:

- "implements the Handoff Protocol v0.1"
- "works with Handoff"
- "a Handoff client for Go"
- "compatible with Handoff by Ωmegas"

This is nominative fair use. You do not need to ask, and you do not owe us attribution beyond what
the licences already require.

## Granted with a condition: the conformance mark

**"Handoff-compatible"** may be claimed for a specific protocol version **only if you have published
a passing conformance run for that version.**

Concretely: run `handoff-conformance` against your implementation, publish the output — the case
list, the pass/fail per case, the suite version, and the date — somewhere a reader can find it, and
link to it from wherever you make the claim. Anyone can rerun it against your public endpoint and
check.

- The claim is **per version**. "Handoff-compatible" with no version is not a claim we recognise.
- The claim **expires** when you ship a change that would fail the suite. Rerun and republish.
- A **fork** that passes the suite may claim conformance. Forks are permitted by the licence and
  conformance is a test result, not a favour. See the renaming requirement below.

This is deliberately stronger than a word mark: it is tied to a test that a skeptic can rerun. We
would rather the phrase mean something than own it.

## Not granted

- **Product or service names containing "Ωmegas"**, or names confusingly similar to it.
- **The Ω mark**, in any form: as your logo, as part of a logo, as an app icon, in a favicon.
- **The form "Handoff by …"** with a party other than Ωmegas — that construction names the origin of
  this project specifically.
- **Any implication of endorsement, affiliation, partnership, or official status** that does not
  exist. Including "official", "certified by", or a badge styled to look like one we issued.
- **Domain names, social handles, or package names** whose evident purpose is to be mistaken for
  ours. Package names in the `handoff*` family on public registries are the sharp case: publishing
  `handoff-core` on a registry where we have not is not a licence violation, but it is a trademark
  problem and we will ask you to transfer or rename it.

## Forks

Forking is permitted by the licence and we are not going to make it awkward. Two requirements:

1. **Rename the distribution.** Your fork needs its own name for its packages, its binaries, and its
   service. You may state factually that it is "a fork of Handoff" and describe what you changed.
2. **Do not imply we maintain it.** Point issue trackers, support, and security contacts at
   yourself.

If your fork passes the conformance suite for a version, claim it — that is the mark working as
intended.

## Questions, and how we enforce

Ask at `trademarks@omegas.dev` before guessing. If a use is close to the line, we would rather tell
you yes in an email than object after you have printed stickers.

Enforcement is proportionate and starts with a conversation. The uses we will actually pursue are
the ones that mislead someone about who stands behind a piece of software — a false conformance
claim, a false endorsement, or a name chosen to be confused with ours. We are not interested in
policing the English word "handoff".
