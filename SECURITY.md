# Security policy

## Reporting a vulnerability

**Do not open a public issue for a security problem.**

Report privately through either channel:

- **GitHub Security Advisories** — [Report a
  vulnerability](https://github.com/OmegaAgent/handoff/security/advisories/new). Preferred: it gives
  us a private fork to develop the fix in and issues the CVE at the end.
- **Email** — `security@omegas.dev`.

Please include: the version or commit, what an attacker gains, the smallest reproduction you have,
and whether you have already disclosed it anywhere.

## What to expect

| Stage | Target |
|---|---|
| Acknowledgement that a human has read it | 3 business days |
| Initial assessment, including whether we agree it is a vulnerability | 10 business days |
| Fix or a dated plan | 90 days from the acknowledgement |

**Coordinated disclosure.** We ask you to hold public disclosure until a fix is released or 90 days
have passed, whichever comes first. If the issue is being actively exploited, tell us and we will
compress that timeline rather than hold you to it.

We will credit you in the advisory unless you ask us not to. There is no bug bounty; this is an
unfunded open-source project and pretending otherwise would waste your time.

## Scope

**In scope** — anything that breaks a security property the protocol claims:

- Forging, replaying, or altering a **receipt** so it verifies against a key it should not.
- Making one human answer authorize **more than one effect**, or an effect other than the one it was
  shown against. Single-use consumption is a protocol guarantee, not an implementation detail.
- Reading, resolving, or cancelling a request across a **tenant boundary**.
- Resolving a request **without being the intended responder**, including through a guessable or
  leaked identifier.
- Forging an **outbound callback signature**, or a verifier that accepts one it should not.
- Anything in `spec/` whose text, if implemented exactly as written, produces one of the above. A
  vulnerability in the specification is the most valuable report we can receive.
- Credentials, tokens, or personal data committed to this repository or leaking into its build
  artifacts, logs, or CI output.

**Out of scope:**

- `examples/night-hack/**`. That directory is preserved prior art from a four-hour hackathon build.
  It is not the reference implementation and it is not maintained. **A deployment of it has existed
  at `handoff.omegas.dev`**, with no authentication — its own docs say anyone holding the URL can
  ring a phone — so treat that hostname as a running hackathon demo rather than as this project,
  and do not assume it is gone until someone has checked. Its
  weaknesses are documented rather than fixed — see `examples/night-hack/README.md`. Reports about
  it are welcome as issues, not as security advisories.
- Denial of service through raw volume against a deployment you control.
- Missing hardening headers on a page you are self-hosting.
- Findings from an automated scanner with no demonstrated impact.
- Social engineering of maintainers or contributors.

## Running a deployment safely

Two properties this project tests are **inert unless the deployment is configured for them**. Both
are stated here because a guarantee that holds only under conditions nobody is asked to create is
not a guarantee.

**Run `handoffd` as a role that cannot bypass row-level security.** Twenty of the twenty-one
`handoff_*` tables have RLS enabled and forced — every one that carries a `tenant_ref`; the
exception is `handoff_migrations`, which is a log of applied migration numbers and holds no
tenant's data. **A superuser, or any role with `BYPASSRLS`, ignores every policy** — and the
development harness in `core/dev/` defaults to exactly such a role, because it also creates and
drops databases.

Give the service role **ownership of its own schema, and neither `SUPERUSER` nor `BYPASSRLS`** —
in practice, `CREATE DATABASE handoff OWNER handoff_app`. Ownership is not a relaxation: the
policies are installed with `FORCE ROW LEVEL SECURITY`, which keeps the owner subject to them. It
is a requirement, because `handoffd` applies its migrations on every start, so a role holding only
`SELECT, INSERT, UPDATE, DELETE` cannot start the process at all — it fails on the first DDL
statement with `permission denied for schema public`. An earlier revision of this file asked for
exactly that grant set; it was never run, and it does not work.

**What the policy does and does not do.** The predicate is
`COALESCE(current_setting('handoff.tenant_ref', true), '') = '' OR tenant_ref = current_setting(…)`:
it restricts to the named tenant, and it **passes when no tenant has been named**. That second half
is deliberate and cannot be removed. `handoffd` authenticates by resolving a credential to a
tenant (§4.1), so the query that discovers the tenant is by construction unable to name it; a
fail-closed policy on `handoff_principals` makes every authenticated request answer `401`. The
cross-tenant sweeps that settle expired requests are in the same position.

So RLS catches **a query that named its tenant and then lost its `WHERE tenant_ref = …`**. It does
not catch a query that never named a tenant at all. Nearly every request-scoped path names one —
`every_request_scoped_path_names_its_tenant` measures this by tightening the policy to fail closed
and driving the API — and the exceptions it currently pins are `GET /deliveries/{id}` and the
idempotency record written by a keyed `POST`. Both carry their own tenant predicate, which is the
primary defence and is present regardless; what they lack is the second line under it.

**Export the receipt chain head somewhere you do not control.** Height contiguity detects alteration
anywhere in a tenant's chain and excision from the middle. It cannot detect **truncation of the
tail**: deleting the newest receipts leaves a shorter chain that verifies perfectly, because nothing
remains to point at what was removed (§9.4). That is inherent to an unanchored hash chain. An
exported head is the anchor that makes truncation visible, and it is the only thing that does — a
deployment that never records its head outside its own database has no evidence against a party who
can delete its newest rows.

## Handling secrets in this repository

The protocol's whole subject matter is authorization, so a leaked identifier here is not a
housekeeping problem.

- **A request identifier can be a capability.** In some designs, possession of the request id is
  what lets a responder act. Treat request ids in logs, screenshots, and issue reports as sensitive
  unless the deployment's design says otherwise. Do not paste one from a live system into a public
  issue.
- **Never commit** `.env`, `*.local.md`, viewer or live-view tokens, API keys, or anything matching
  `sk-`/`Bearer <literal>`. `.gitignore` covers the common cases; it is a safety net, not a policy.
- **Screenshots are a leak surface.** Redact request ids, tokens, URLs bearing a `token=` query
  parameter, and any address that resolves.
- **If you do commit a secret**: rotate it first, then tell us. Rotation is the fix; removing it
  from history is cleanup. Do not rewrite this repository's published history without asking — the
  MIT grant on already-published code and the DCO trail both depend on it staying intact.

## Cryptography

Report a weakness in the receipt or callback signature scheme through the private channel above,
even if you think it is theoretical. The signature scheme and its test vectors are normative: a
break in either is a spec-level issue with a major version bump attached, and we would rather learn
about it before someone builds on it.
