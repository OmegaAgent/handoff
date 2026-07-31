# `@handoffproto/types`

TypeScript declarations for the [Handoff protocol](../../spec/openapi.yaml) — the wire
contract for asking a human to decide or act, and for getting the answer back to a process
that may have died in the meantime.

**Types only.** No runtime code, no dependencies, nothing to import at run time. The package
ships one `.d.ts` file, one zero-dependency Node script, and this README.

```ts
import type {
  RaiseRequest,
  Request,
  Signal,
  Receipt,
  Authorization,
  ErrorCode,
} from "@handoffproto/types";

async function raise(body: RaiseRequest): Promise<Request> {
  const res = await fetch("https://handoff.example.com/v1/requests", {
    method: "POST",
    headers: {
      "content-type": "application/json",
      "Idempotency-Key": "refund-8821-attempt-1",
      authorization: `Bearer ${process.env.HANDOFF_API_KEY}`,
    },
    body: JSON.stringify(body),
  });
  return res.json() as Promise<Request>;
}
```

## What is in here

`index.d.ts` contains:

1. **One exported type per schema** in `spec/openapi.yaml` → `components/schemas`, named
   exactly as the OpenAPI document names it (`RaiseRequest`, `Request`, `Signal`,
   `Decision`, `Receipt`, `ReceiptActor`, `Authorization`, `RedeemRequest`, `ErrorCode`,
   `ErrorBody`, `Meta`, …). These live inside a delimited region:

   ```ts
   // #region openapi:components/schemas
   …
   // #endregion openapi:components/schemas
   ```

   The region is what the drift check compares against the spec. Nothing else belongs in it.

2. **Named unions for enums the document repeats inline** (`Urgency`, `Liveness`,
   `AuthStrength`, `GrantScope`, `WaiterState`, `DeliveryGrade`, `Disposition`, …). These are
   *not* OpenAPI schemas — they exist so the mirror stays readable, and they sit outside the
   region.

3. **Per-operation request/response aliases** derived from `paths:`, named after each
   operation's `operationId` (`RaiseRequestBody` / `RaiseRequestResponse`,
   `AnswerRequestBody` / `AnswerRequestResponse`, `RedeemAuthorizationBody` /
   `RedeemAuthorizationResponse`, `GetMetaResponse`, …), plus query-parameter interfaces for
   the paginating and long-polling operations (`ListRequestsQuery`, `ListReceiptsQuery`,
   `GetRequestQuery`, `PollWaiterSignalsQuery`). Also outside the region.

### Transcription rules

These are applied uniformly, and they are the rules a reviewer should check against:

| OpenAPI | TypeScript |
| --- | --- |
| listed in `required:` | `x: T` |
| not listed in `required:` | `x?: T` |
| `anyOf: [X, {type: "null"}]` | `X \| null` |
| required **and** nullable | `x: T \| null` |
| optional **and** nullable | `x?: T \| null` |
| `enum: [a, b]` | `"a" \| "b"` — every member transcribed |
| `const: 1` | `1` |
| `additionalProperties: true` | index signature `[key: string]: unknown` |
| `additionalProperties: false` | closed shape, no index signature |
| property with no `type` | `unknown` |
| `format: date-time` / `uri` / `duration` | `string` (the format is in the doc comment) |

"Nullable but required" (`x: T | null`) and "optional" (`x?: T`) are **different facts** and
are encoded differently. `null` on the wire means "the server computed this and the answer is
nothing"; absence means the server said nothing at all. Collapsing them would erase, for
example, the difference between `attempt_expires_at: null` ("no attempt is armed") and an
older server that does not report attempt clocks.

Property names are **verbatim snake_case**. Nothing is camelCased — these types describe the
JSON on the wire, not an ergonomic client object.

## The honest part: this file is hand-maintained

No code generator is vendored in this repository, and this package installs nothing. So
`index.d.ts` was written by hand against `spec/openapi.yaml`, and **the drift check is what
keeps it honest**. Until a generator is vendored, treat the drift check — not good intentions —
as the mechanism.

```console
$ npm run check
check-drift: OK — 87 schemas in spec/openapi.yaml all have an exported type in sdk/types/index.d.ts.
```

`scripts/check-drift.mjs` re-reads `spec/openapi.yaml` with an indentation-aware regex (no YAML
parser, no dependencies), enumerates the schema names under `components/schemas`, and asserts
that the set is exactly the set of exported types inside the mirrored region. It exits non-zero
with a readable list of missing and extra names on failure, and it resolves the spec path
relative to its own location, so it runs from any working directory.

What the drift check **does** catch: a schema added, removed, or renamed in the spec.

What it **does not** catch: a property added to an existing schema, a `required:` list edited,
an enum member added, a type changed from string to integer. Those need the re-check below.

## Re-checking after the spec changes

```console
# from sdk/types/
npm run check                      # schema-name drift (fast, run this always)
npm run typecheck                  # tsc --noEmit --strict index.d.ts
```

Both must pass. Then, for anything the name check cannot see:

1. Read the diff of `spec/openapi.yaml` under `components/schemas` and `paths`.
2. For every touched schema, re-transcribe **the whole schema**, not just the changed line:
   the `required:` list, every property, every `enum`, every `anyOf: […, {type: "null"}]`.
   Apply the table above.
3. If a schema was added or renamed, add or rename the exported type **inside** the
   `// #region openapi:components/schemas` block. Keep document order.
4. If `paths` gained an operation, add `…Body` / `…Response` aliases named after its
   `operationId`, outside the region.
5. Re-run both commands, then re-check the canonical fixtures (below).

### Verifying against the canonical fixtures

`spec/fixtures/*.json` are the protocol's canonical example payloads. The strongest available
check is to let `tsc` compare them against these declarations with `satisfies`, which gives
both literal-type checking and excess-property checking:

```ts
// scratch file, not part of the package
import type { Request, Signal, Receipt, Authorization, Meta } from "@handoffproto/types";

export const r = { /* paste spec/fixtures/02-request-created.json */ } satisfies Request;
export const s = { /* paste spec/fixtures/05-signal-answered.json  */ } satisfies Signal;
export const c = { /* paste spec/fixtures/08-receipt-decision.json */ } satisfies Receipt;
export const a = { /* paste spec/fixtures/10-authorization.json    */ } satisfies Authorization;
export const m = { /* paste spec/fixtures/18-meta.json             */ } satisfies Meta;
```

Paste the JSON **inline**. A `resolveJsonModule` import widens `"pending"` to `string` and the
check silently proves nothing.

**When a fixture and the OpenAPI document disagree, do not loosen the type to make the error go
away.** The type mirrors `spec/openapi.yaml`; a disagreement is a finding about the spec, and it
belongs in an issue against `spec/`, not in a widened union here.

## Regenerating, once a generator is vendored

The intended end state is that `index.d.ts` is generated, not typed:

```console
npx openapi-typescript ../../spec/openapi.yaml -o index.d.ts
```

That is **not** how this file was produced and that command is not wired into `scripts` —
this package has no dependencies and does no network access. When a generator is vendored,
whoever does it should:

- keep the exported names identical to the schema names, so `check-drift.mjs` keeps working;
- keep (or re-emit) the `// #region openapi:components/schemas` markers;
- keep the per-operation aliases, which a generator will not produce in this shape.

Until then: hand-edit, run `npm run check` and `npm run typecheck`, and re-check the fixtures.

## Notes for implementers

A few contract facts that the types encode but that are easy to lose:

- **A `secret` field's answer value is `{"provided": true}` and nothing else.** The value went
  out of band to the declared sink (`POST /sinks/{sink_ref}/values`). A raw value posted against
  a `secret` field is `422 answer_validation_failed`. `AnswerRequest.values` is
  `Record<string, unknown>` because the shape is per-field, not because anything goes.
- **`dispatched` is not evidence a person received anything.** It means our transport accepted
  it. See `DeliveryGrade`.
- **Reading a signal does not consume it.** `GET /waiters/{waiter_ref}/signals` returns unacked
  signals; `POST /signals/{signal_id}/ack` is what consumes one.
- **A `GrantHandle` is a pointer, not a credential.** Holding it confers nothing without an
  authenticated resolve. `GrantTransport.url` is the one resolvable address in the protocol and
  MUST NOT be persisted.
- **Ids are tenant-scoped and are never an authorization.** `Request.surface_url` is a locator:
  opening it prompts for authentication.
- **Clients MUST ignore unknown response fields.** These declarations describe version 0.1;
  additive server changes are not breaking.

## Licence

MIT. See [`LICENSE`](../LICENSE).
