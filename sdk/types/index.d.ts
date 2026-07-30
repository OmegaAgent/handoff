/**
 * `@handoffproto/types` — TypeScript declarations for the Handoff protocol.
 *
 * HAND-WRITTEN mirror of `spec/openapi.yaml` (`info.version: 0.1.0`,
 * `Meta.protocol_version: "0.1"`). There is no code generator vendored in this
 * repository yet, so this file is maintained by hand and kept honest by
 * `scripts/check-drift.mjs`, which re-reads the OpenAPI document and asserts
 * that every schema under `components/schemas` has an exported type here.
 *
 * Transcription rules, applied uniformly:
 *   - Property names are verbatim snake_case. Nothing is camelCased.
 *   - `required:` decides `x: T` vs `x?: T`. These are different facts and are
 *     encoded differently: a nullable-but-required property is `x: T | null`;
 *     an absent-allowed property is `x?: T`.
 *   - `anyOf: [X, {type: "null"}]` becomes `X | null`.
 *   - `enum` becomes a string-literal union with every member transcribed.
 *   - `additionalProperties: true` becomes an index signature.
 *   - `additionalProperties: false` (or absent, per generator convention)
 *     becomes a closed shape with no index signature.
 *   - `const` becomes a single-member literal type.
 *
 * Timestamps are RFC 3339 strings in UTC. Durations are ISO-8601 strings.
 * Ids are `<prefix>_<26-character Crockford base32 ULID>`.
 */

// ---------------------------------------------------------------------------
// Named unions for enums that the OpenAPI document repeats inline.
// These are NOT schemas in `components/schemas`; they exist only so the mirror
// below stays readable. The drift check deliberately ignores them.
// ---------------------------------------------------------------------------

/** How hard to try. The agent declares urgency; Handoff decides the channel. */
export type Urgency = "low" | "normal" | "high" | "critical";

/**
 * `durable` — the wait survives this process. `leased` — the wait is a live
 * client process holding a poll.
 */
export type Liveness = "durable" | "leased";

/** Whether the agent must redeem an authorization before the effect. */
export type RequestMode = "advisory" | "gated";

/** Under `strict`, an answer echoing a stale `rendered_digest` is rejected. */
export type PresentationBinding = "advisory" | "strict";

/** What to do when the waiter goes terminal or its lease lapses. */
export type OnWaiterTerminal = "keep" | "cancel";

/** How firmly the answerer's identity must be established. */
export type AuthStrength = "link_only" | "session" | "reauth" | "mfa";

/** Minimum role in the owning tenancy. */
export type MinRole = "viewer" | "editor" | "admin";

/**
 * `view` is frames out, no input in. `drive` is full control and requires
 * strictly higher authority.
 */
export type GrantScope = "view" | "drive";

/** The waiting side's lifecycle. `orphaned` is visible on purpose. */
export type WaiterState =
  | "armed"
  | "signalled"
  | "delivering"
  | "acked"
  | "orphaned"
  | "released";

/**
 * Graded evidence, and the grades are NOT interchangeable. `dispatched` means
 * our transport accepted it — it is not evidence a person received anything.
 * `delivered` means the channel says it reached the person's endpoint. `seen`
 * means the person opened the request surface authenticated. `acted` means the
 * person answered through this delivery.
 */
export type DeliveryGrade = "dispatched" | "delivered" | "seen" | "acted";

/** What the human is asked to do with the request. */
export type Disposition = "decide" | "delegate" | "unable";

// #region openapi:components/schemas
// ---------------------------------------------------------------------------
// One exported type per schema in `components/schemas`, in document order.
// Names match the OpenAPI document exactly.
// ---------------------------------------------------------------------------

// ------------------------------------------------------------- primitives

/**
 * `<prefix>_<26-character Crockford base32 ULID>` — time-sortable and safe to
 * log. Crockford base32 omits `I`, `L`, `O` and `U`.
 *
 * Pattern: `^[a-z]{2,4}_[0-9A-HJKMNP-TV-Z]{26}$`
 */
export type Id = string;

/**
 * A human-intervention request.
 *
 * Pattern: `^req_[0-9A-HJKMNP-TV-Z]{26}$`
 */
export type RequestId = string;

/**
 * An immutable receipt.
 *
 * Pattern: `^rcpt_[0-9A-HJKMNP-TV-Z]{26}$`
 */
export type ReceiptId = string;

/**
 * A spendable authorization.
 *
 * Pattern: `^auth_[0-9A-HJKMNP-TV-Z]{26}$`
 */
export type AuthorizationId = string;

/**
 * One attempt-bearing send to one target on one channel.
 *
 * Pattern: `^dlv_[0-9A-HJKMNP-TV-Z]{26}$`
 */
export type DeliveryId = string;

/**
 * One queued notification to a waiter.
 *
 * Pattern: `^sig_[0-9A-HJKMNP-TV-Z]{26}$`
 */
export type SignalId = string;

/**
 * An opaque capability grant handle. It is a database pointer, NOT a
 * credential: holding it confers nothing without an authenticated resolve.
 * Unlike other ids a handle MUST come from a cryptographic RNG rather than a
 * clock, so it cannot be guessed or recomputed from a leaked platform secret.
 *
 * Pattern: `^hg_[0-9A-HJKMNP-TV-Z]{26}$`
 */
export type GrantHandle = string;

/**
 * A short-lived, lease-bound session produced by resolving a grant.
 *
 * Pattern: `^hs_[0-9A-HJKMNP-TV-Z]{26}$`
 */
export type GrantSessionRef = string;

/**
 * A runtime-owned destination for `secret`-typed values. It never appears in a
 * receipt.
 *
 * Pattern: `^snk_[0-9A-HJKMNP-TV-Z]{26}$`
 */
export type SinkRef = string;

/**
 * Proof that the acking client is the waiter the signal was enqueued for.
 * Never a capability for anything else.
 *
 * Pattern: `^rt_[0-9A-HJKMNP-TV-Z]{26}$`
 */
export type ResumeToken = string;

/**
 * A principal: `usr_` a person, `sa_` a service account, `org_` an
 * organization. A service account has no user identity — that is what makes
 * `requester_may_not_answer` decidable by type.
 *
 * Pattern: `^(usr|org|sa)_[0-9A-HJKMNP-TV-Z]{26}$`
 */
export type PrincipalId = string;

/**
 * The organization that owns the resource. Always derived from the credential,
 * never from a body.
 *
 * Pattern: `^org_[0-9A-HJKMNP-TV-Z]{26}$`
 */
export type OrgId = string;

/**
 * The caller's opaque grouping key for a unit of agent work — a run id, a
 * thread id, a session id. Stored and matched byte-for-byte; the server never
 * parses it and never switches on it. 1–512 characters.
 */
export type WaiterRef = string;

/**
 * An ISO-8601 duration of EXACT length — `PT15M`, `PT4H`, `P1D`, `P2W`. Years
 * and months are not permitted, because their length depends on when the clock
 * starts. For retention windows see `CalendarDuration`.
 *
 * Pattern: `^P(?!$)(\d+W)?(\d+D)?(T(?=\d)(\d+H)?(\d+M)?(\d+S)?)?$`
 */
export type Duration = string;

/**
 * An ISO-8601 duration that MAY use years and months, resolved against a
 * calendar. Used ONLY for retention windows. It MUST NOT be used for a TTL, an
 * attempt window, a grant expiry, a session lease, or a ladder delay.
 *
 * Pattern: `^P(?!$)(\d+Y)?(\d+M)?(\d+W)?(\d+D)?(T(?=\d)(\d+H)?(\d+M)?(\d+S)?)?$`
 */
export type CalendarDuration = string;

/**
 * A hash, algorithm-prefixed so it can be migrated without ambiguity.
 *
 * Pattern: `^[a-z0-9-]+:[0-9a-f]{32,128}$`
 */
export type Digest = string;

// ------------------------------------------------------------------ raise

/**
 * Everything a caller declares when it asks for a human. Note what is NOT
 * here: no channel, no recipient address, no request `kind`. The agent
 * declares urgency and what it needs; routing is org policy resolved
 * server-side and snapshotted onto the request, so a policy edit mid-flight
 * cannot retroactively change what happened.
 */
export interface RaiseRequest {
  waiter_ref: WaiterRef;
  /** Default `durable`. */
  liveness?: Liveness;
  /** Default `normal`. */
  urgency?: Urgency;
  prompt: Prompt;
  requires: Requires;
  /**
   * How long the ask stays worth answering. ABSENT MEANS THE REQUEST NEVER
   * EXPIRES — an accidental deadline silently converts a decision into a
   * policy outcome.
   */
  ttl?: Duration;
  ttl_policy?: TtlPolicy;
  /**
   * The attempt window — "a specific human is expected to be doing this right
   * now". Re-armed fresh on each progressive step. Default `PT15M`.
   */
  attempt_ttl?: Duration;
  /** `keep` defaults for `durable`; `cancel` defaults for `leased`. */
  on_waiter_terminal?: OnWaiterTerminal;
  routing?: Routing;
  /** Default `advisory`. */
  mode?: RequestMode;
  /** Default `advisory`. */
  presentation_binding?: PresentationBinding;
  /**
   * Ask-once key. While a request with this key is `pending`, a second raise
   * returns that request instead of paging anyone again. Max 255 characters.
   */
  dedupe_key?: string;
  /**
   * Level 2 (`continuation` extension). An opaque pointer the runtime hands
   * back to itself. Returned byte-identical in the signal and stored nowhere
   * else. Handoff never dereferences it. Max 2048 characters.
   */
  resume_ref?: string;
  /**
   * Level 2 (`continuation` extension). Base64 continuation state, returned
   * byte-identical in the signal. Capped at roughly 64 KiB, and a server MUST
   * NOT log it. Max 87400 characters.
   */
  resume_payload?: string;
  callback?: Callback;
  /**
   * Caller-owned annotations, stored and returned verbatim. THE CORE NEVER
   * SWITCHES ON ANYTHING IN HERE — including a `hint` key. Keys prefixed `x-`
   * are reserved for vendor extensions and are never interpreted.
   */
  metadata?: Record<string, unknown>;
  /**
   * Route this request to a sandbox destination instead of to real people. The
   * only requests a `test`-environment key may raise. Default `false`.
   */
  test_mode?: boolean;
}

/**
 * What the human reads. Markdown for prose; structured `evidence` for anything
 * they must check.
 */
export interface Prompt {
  /**
   * One line, and it must be answerable on its own — it is what arrives on a
   * lock screen, in a push notification, and in a one-line chat preview.
   * 1–200 characters.
   */
  title: string;
  /** Markdown. Rendered by the surface; never interpreted by the core. */
  body?: string;
  /** Max 50 items. */
  evidence?: Evidence[];
}

/**
 * One piece of supporting material. Exactly one of `url`, `value` or `ref`
 * carries the content.
 */
export interface Evidence {
  /**
   * How to render it. Unknown kinds are rendered as a labelled link or
   * ignored — never as an error.
   */
  kind: "link" | "table" | "image" | "document" | "text";
  /** What this evidence is, in the human's words. Max 200 characters. */
  label: string;
  /**
   * An external locator. Opening it is the reader's own authenticated action;
   * it is never a capability.
   */
  url?: string;
  /** Inline content — a table object, a string, a structured document. */
  value?: unknown;
  /** An opaque handle to content held elsewhere, resolved by the surface. */
  ref?: string;
}

/**
 * The three orthogonal declarations that replace a request `kind` enum: the
 * shape of the answer, the capabilities the human must be handed to be *able*
 * to answer, and who is entitled to answer. Every use case is a different
 * population of these three, and none of them adds a branch to a conforming
 * server.
 *
 * Additional keys are stored verbatim; `x-` prefixed keys are never
 * interpreted.
 */
export interface Requires {
  /**
   * Envelope version. A server that does not understand this value MUST reject
   * the raise with `400 unsupported_requires_version` and create nothing.
   */
  v: 1;
  answer?: AnswerSpec;
  /** What the human must be handed in order to answer. Max 16 items. */
  capabilities?: CapabilityDeclaration[];
  authority?: Authority;
  [key: string]: unknown;
}

/**
 * The shape of a valid answer. One declaration; one renderer; no per-vendor
 * code anywhere.
 */
export interface AnswerSpec {
  /**
   * MAY BE EMPTY, AND EMPTY IS MEANINGFUL: it means the whole request is an
   * attestation — there is nothing to type, the human does the thing and says
   * so. Max 64 items.
   */
  fields?: Field[];
  value_sink?: ValueSink;
}

/**
 * Declared, never assumed. Where `secret`-typed values go, on a channel the
 * runtime owns. The core knows only that such values exist and that the answer
 * carries `{"provided": true}`.
 */
export interface ValueSink {
  /** Opaque provider name. The core looks it up; it never matches the string. */
  provider?: string;
  /** Opaque operation name meaningful to the provider only. */
  op?: string;
  /**
   * An opaque provider resource reference, NOT a Handoff `snk_` identifier. It
   * MUST NOT be a URL and MUST NOT embed a credential. `Field.sink_ref` is the
   * typed `snk_…` handle; this is the runtime's own coordinate.
   */
  ref: string;
}

/**
 * One thing the human is asked for. The type set is closed and versioned: an
 * unknown `type` is `400 unsupported_field_type`, because a field the renderer
 * cannot draw is a field the human cannot answer.
 */
export interface Field {
  /**
   * Stable key in the answer's `values` object. Lowercase snake case.
   *
   * Pattern: `^[a-z][a-z0-9_]{0,63}$`
   */
  name: string;
  /** What the human sees. Defaults to a humanized `name`. */
  label?: string;
  /**
   * `choice` carries the option `id` (an array when `multi`). `text`,
   * `number`, `boolean` carry their value. `secret` CARRIES NOTHING in the
   * answer — the value goes to the sink and the answer says
   * `{"provided": true}`. `attestation` carries `true` only. `document`
   * carries a structured value validated against `schema_ref`. `file_ref`
   * carries an opaque handle, never bytes.
   */
  type:
    | "choice"
    | "text"
    | "number"
    | "boolean"
    | "secret"
    | "attestation"
    | "document"
    | "file_ref";
  /**
   * A missing required field is `422 answer_validation_failed`, named in
   * `fields`. Default `false`.
   */
  required?: boolean;
  /** One line of guidance shown near the input. */
  help?: string;
  /**
   * For `choice`. An answer outside this set is a validation failure, not a
   * free-text fallback. Max 100 items.
   */
  options?: FieldOption[];
  /** For `choice` — the answer is an array of option ids. Default `false`. */
  multi?: boolean;
  /** For `text`. Enforced server-side, not only in the surface. */
  max_len?: number;
  /** For `number`, inclusive lower bound. */
  min?: number;
  /** For `number`, inclusive upper bound. */
  max?: number;
  /**
   * Pre-filled value. For `document` this is what the human edits — the
   * review-and-correct case, where the agent proposes and the person amends.
   */
  initial?: unknown;
  /** For `document`. The answer is validated against this schema. */
  schema_ref?: string;
  sink_ref?: SinkRef;
}

/**
 * One selectable option. `id` is what the answer carries; `label` is what the
 * human reads.
 */
export interface FieldOption {
  id: string;
  label: string;
}

/**
 * Who is entitled to answer, declared ON THE REQUEST rather than implied by
 * whichever endpoint happened to be called. A delivery channel never confers
 * authority: receiving the page and being allowed to answer are separate facts.
 */
export interface Authority {
  /** A lower-privileged answerer is `403 insufficient_authority`. */
  min_role?: MinRole;
  /**
   * `link_only` is defined so deployments that knowingly accept it can record
   * it honestly — the receipt says `actor.type = "anonymous_link"` — but a
   * deployment MAY refuse it with `403 auth_strength_not_permitted`. Default
   * `session`.
   */
  auth_strength?: AuthStrength;
  /**
   * Who this is for. Empty means "anyone who satisfies the rest of this
   * block". Max 50 items.
   */
  assignees?: Target[];
  /**
   * How many distinct principals must answer before the request settles. `1`
   * is the only value a Level 1 server must support. Default `1`.
   */
  quorum?: number;
  /**
   * Always true, and stated rather than assumed: the principal that raised a
   * request may never answer it. Enforced by principal TYPE.
   */
  forbid_requester?: true;
  /** Why this authority is required, shown to the human. */
  reason?: string;
}

/**
 * Who to reach. One union, resolved through a single
 * `resolve_targets(target) -> [principal]` step, so adding a targeting concept
 * never adds a branch. `rotation` resolves at rung-fire time, not at raise
 * time, so an escalation reaches whoever is actually on call.
 */
export interface Target {
  kind: "principal" | "role" | "group" | "rotation" | "anyone";
  /** Opaque to the protocol; meaningful to the deployment's directory. */
  value: string;
}

/**
 * A per-request override of the org's escalation policy. Requires the
 * `handoff:requests:route` scope. Whatever ladder applies is snapshotted onto
 * the request at raise time.
 */
export interface Routing {
  /** Rung-0 targets. Max 50 items. */
  targets?: Target[];
  /**
   * Ordered rungs. A RUNG MINTS DELIVERIES, NEVER A NEW REQUEST — one
   * intervention, one decision, one receipt, however many people were tried.
   * Max 10 items.
   */
  ladder?: RoutingRung[];
}

/** One step of an escalation ladder, fired `after` the request was raised. */
export interface RoutingRung {
  after: Duration;
  /**
   * Channel names are an OPEN VOCABULARY: the core carries the string and
   * looks up an adapter. At least 1 item.
   */
  channels: string[];
  to?: Target;
}

/**
 * What happens when `expires_at` arrives. The protocol's bias is to fail
 * toward a typed terminal answer, never toward silence — and when the answer
 * must be guessed, guess "no".
 */
export interface TtlPolicy {
  /**
   * `escalate` advances the ladder and extends the TTL. `expire_and_deny`
   * settles as `expired` with `effective: "deny"`. `default` settles as
   * `expired` carrying `default_answer`, and its receipt records
   * `actor.type = "policy"` so no audit ever mistakes it for consent. `park`
   * never expires and re-reminds on a cadence.
   */
  on_expiry: "escalate" | "expire_and_deny" | "default" | "park";
  /**
   * Required when `on_expiry` is `default`. MUST BE DECLARED AT RAISE TIME,
   * before anyone knew the human would go quiet — that is what makes it a
   * pre-agreement rather than a convenient assumption made after the fact.
   */
  default_answer?: Record<string, unknown>;
  /** For `park` only. How often to re-page while the request stays open. */
  reminder_every?: Duration;
}

/**
 * Where to POST signals for runtimes whose wait lives server-side, so they
 * never poll. A callback carries ids and typed values — never a grant URL,
 * never a secret value. A `2xx` marks the callback *dispatched*; the signal is
 * still not consumed until it is acked.
 */
export interface Callback {
  /** HTTPS endpoint. Signed per delivery. */
  url: string;
  /**
   * Handle for the signing secret. Two secrets may be active during a rotation
   * overlap.
   */
  secret_ref?: string;
}

// ----------------------------------------------------------- capabilities

/**
 * What the human must be handed, declared as an opaque handle plus enough
 * description to explain it. NO RESOLVABLE ADDRESS EVER APPEARS HERE — not a
 * URL, not an endpoint, not a token. The handle is exchanged for a live
 * session at the authenticated resolve endpoint, by the human's own client.
 */
export interface CapabilityDeclaration {
  handle: GrantHandle;
  /**
   * Open vocabulary of capability kinds (for example `interactive_surface`).
   * Unknown types are `400 unsupported_capability_type`. `GET /meta` lists
   * what a deployment accepts.
   */
  type: string;
  /** Scope is enforced at the grant, not by a client-side attribute. */
  scope: GrantScope;
  /** Opaque provider name. The core never matches on the string. */
  provider?: string;
  /** Opaque provider resource id, handed straight back to the provider. */
  resource_ref?: string;
  /** What this is, in the human's words. */
  label?: string;
  /** Why the human is being handed it, so accepting is an informed act. */
  purpose?: string;
  /**
   * `true` means the request is answerable without ever resolving this
   * capability — the escape hatch pattern. Default `false`.
   */
  optional?: boolean;
  /** How long the grant stays resolvable. */
  ttl?: Duration;
  /**
   * Digest of the `blast_radius` a human will be shown. The resolve call
   * echoes it back as `accepted_blast_radius_digest`, so a human can never be
   * handed something other than what they read.
   */
  blast_radius_digest?: Digest;
}

/**
 * The declaration plus everything a surface needs to let a human decide
 * whether to accept: the FULL blast radius, the expiry, the binding, and any
 * provider constraints. Readable only with a human session.
 */
export interface CapabilityGrant {
  handle: GrantHandle;
  request_id?: RequestId;
  /** The capability kind, as declared on the request. */
  type: string;
  /**
   * The maximum scope this grant can produce. A session may request a subset,
   * never a superset.
   */
  scope: GrantScope;
  provider?: string;
  resource_ref?: string;
  label?: string;
  purpose?: string;
  optional?: boolean;
  ttl?: Duration;
  blast_radius: BlastRadius;
  blast_radius_digest: Digest;
  /** After this instant the grant is `410 capability_expired`. */
  expires_at?: string;
  /** Non-null means no further sessions, ever. */
  revoked_at?: string | null;
  /**
   * How many distinct people may hold this grant at once. `1` is the default
   * and means the second person to resolve gets `409 grant_already_held`.
   */
  max_holders?: number;
  /** The principal this grant pinned to on first successful resolve. */
  bound_principal_id?: PrincipalId | null;
  /**
   * Provider-enforced narrowing. THE CORE CARRIES THIS OBJECT AND COMPARES
   * NOTHING INSIDE IT; the provider enforces it.
   */
  constraints?: Record<string, unknown>;
}

/**
 * The scope of consequence a human accepts when they take a capability.
 * Provider-declared, core-carried, human-rendered, receipt-digested. `summary`
 * MUST be shown before the control that resolves the grant.
 */
export interface BlastRadius {
  /** One sentence a non-expert understands. Max 300 characters. */
  summary: string;
  /**
   * The one field the core can COMPARE, which is why it is a closed vocabulary
   * while everything else here is opaque provider text.
   */
  shared_with: "isolated" | "request" | "space" | "org";
  /** How many people's access is implicated. A count, never a roster. */
  principals?: number;
  /**
   * What the surface is signed in as, shown to the human before they accept.
   * Effectively personal data: only the digest goes in the receipt. Max 100
   * items.
   */
  identities?: BlastRadiusIdentity[];
  /** Whether actions taken through this capability can be undone. */
  reversible?: boolean;
  /** Any additional consequence the human should know. */
  note?: string;
}

/** One identity the capability is signed in as. */
export interface BlastRadiusIdentity {
  /**
   * Where the identity applies. Origin-level by default; never a full URL with
   * parameters.
   */
  origin: string;
  /** The account as a human would name it. */
  label?: string;
}

/**
 * Resolve a grant handle into a live session. Sent by the human's own client,
 * with a human session.
 */
export interface CreateGrantSessionRequest {
  /**
   * A subset of the grant's scope. Asking for more than the grant allows is
   * `403 insufficient_authority`. 1–2 items.
   */
  scopes: GrantScope[];
  /**
   * The digest of the blast radius this human was actually shown. A mismatch
   * is `409 blast_radius_mismatch` — required, because "I accepted" must mean
   * "I accepted *this*".
   */
  accepted_blast_radius_digest: Digest;
  /**
   * Further narrowing the human accepts (never widening). Carried to the
   * provider verbatim; the core compares nothing inside it.
   */
  constraints?: Record<string, unknown>;
}

/**
 * A live, leased session. `transport.url` is minted here and nowhere else, is
 * single-session and short-lived, and MUST NOT BE PERSISTED — not in a
 * database, not in a message, not in a log, not in a model's context.
 */
export interface GrantSession {
  session_ref: GrantSessionRef;
  /**
   * What this session may actually do — the intersection of what was asked for
   * and what authority permits.
   */
  scopes: GrantScope[];
  /**
   * The session closes at this instant unless renewed. It bounds how long a
   * revocation takes to bite.
   */
  lease_until: string;
  /** Renew after this many milliseconds, not at the last moment. */
  renew_after_ms?: number;
  blast_radius?: BlastRadius;
  transport: GrantTransport;
  /** The receipt recording that this person took this capability. */
  receipt_id?: ReceiptId | null;
}

/** How to reach the live surface. Ephemeral by construction. */
export interface GrantTransport {
  /** The client picks its connection method from this, not from the URL. */
  kind: "websocket" | "https";
  /**
   * Single-session, expires with the lease. Treat it as a secret in flight and
   * discard it on release; it is the one resolvable address in the entire
   * protocol.
   */
  url: string;
}

/** Optional reason recorded with the revocation. */
export interface RevokeGrantRequest {
  /** Why the grant was pulled. Shown in the audit trail. */
  reason?: string;
}

/**
 * The extended lease. Re-checked against revocation, expiry, binding, and the
 * caller's current role.
 */
export interface RenewResult {
  lease_until: string;
  /** When to renew next. */
  renew_after_ms?: number;
}

// ---------------------------------------------------------------- request

/**
 * Four terminal states, and every one of them produces a typed terminal signal
 * to the waiter. THERE IS NO PATH BY WHICH A REQUEST GOES QUIET.
 */
export type RequestState =
  | "pending"
  | "answered"
  | "expired"
  | "cancelled"
  | "superseded";

/**
 * The full representation of a human-intervention request.
 *
 * Note: unlike its siblings this schema does not declare
 * `additionalProperties: false`, and clients MUST ignore unknown response
 * fields regardless.
 */
export interface Request {
  id: RequestId;
  state: RequestState;
  /** Bumped by every amendment. A receipt records the version the human saw. */
  version: number;
  org_id: OrgId;
  waiter_ref: WaiterRef;
  /** As declared at raise time. */
  urgency?: Urgency;
  /**
   * `attention` — a human is expected to be on it right now. `waiting` — the
   * attempt lapsed and nobody is actively working it. THIS IS A LABEL AND A
   * SORT KEY, NEVER A FILTER: a `pending` request whose attempt lapsed must
   * stay listed and answerable.
   */
  urgency_state?: "attention" | "waiting";
  prompt: Prompt;
  requires: Requires;
  mode?: RequestMode;
  presentation_binding?: PresentationBinding;
  liveness?: Liveness;
  on_waiter_terminal?: OnWaiterTerminal;
  ttl_policy?: TtlPolicy;
  /**
   * The routing ladder as resolved server-side at raise time and SNAPSHOTTED
   * onto the request, so that a policy edit mid-flight cannot retroactively
   * change what happened.
   */
  routing?: Routing;
  created_at: string;
  /** Null when no TTL was declared — the ask waits indefinitely. */
  expires_at?: string | null;
  /**
   * Null until an attempt is armed. Lapsing changes `urgency_state`, never
   * `state`.
   */
  attempt_expires_at?: string | null;
  /** Server clock at the moment the settling write committed. */
  answered_at?: string | null;
  /** The successor, when this request was superseded. */
  superseded_by?: RequestId | null;
  /** Shown to any human who was mid-answer when the request was withdrawn. */
  cancel_reason?: string | null;
  /**
   * A LOCATOR, NOT A CAPABILITY. Opening it prompts for authentication;
   * knowing the URL grants nothing. The unguessable id is never the
   * access-control model.
   */
  surface_url?: string;
  /** Every delivery minted so far, across every rung. */
  deliveries?: Delivery[];
  /** Present once the request is settled; null while `pending`. */
  receipt?: Receipt | null;
  /** Present once an answer minted one; null otherwise. */
  authorization?: Authorization | null;
  waiter?: WaiterView;
  /** Returned verbatim, never interpreted. */
  metadata?: Record<string, unknown>;
}

/**
 * The waiting side, as seen from the request. `orphaned` is visible on
 * purpose: a request whose requester died is still answerable, and a human
 * deserves to know their answer may arrive after the run that asked is gone.
 */
export interface WaiterView {
  state?: WaiterState;
  liveness?: Liveness;
}

/**
 * One page of requests. Ask for the next page by passing `next_cursor` as
 * `cursor`.
 */
export interface RequestList {
  data: Request[];
  has_more: boolean;
  /** Null when `has_more` is false. */
  next_cursor?: string | null;
}

/**
 * Fields present are merged forward; fields absent are untouched. Amending
 * never changes the request id, the waiter, or an in-progress attempt.
 */
export interface AmendRequest {
  prompt?: Prompt;
  requires?: Requires;
}

/** Withdraw the ask. */
export interface CancelRequest {
  /**
   * Shown to a human who is mid-answer, so the surface can explain why it just
   * changed under them. 1–500 characters.
   */
  reason: string;
}

/** Point this request at the request that replaces it. */
export interface SupersedeRequest {
  /** The successor. It must already exist and still be `pending`. */
  by: RequestId;
}

/** Advance the ladder now rather than waiting for the rung timer. */
export interface EscalateRequest {
  /** Jump to a specific rung. Omitted means "the next one". */
  rung?: number;
}

/** Retarget the request at somebody else. */
export interface ReassignRequest {
  to: Target;
  /** Recorded on the request and surfaced to the new target. */
  reason?: string;
}

/** Arm or re-arm the attempt clock. */
export interface AttemptRequest {
  /** Overrides the request's `attempt_ttl` for this window only. */
  ttl?: Duration;
}

// ----------------------------------------------------------------- answer

/**
 * A human's settling write. `values` is keyed by declared field `name`;
 * anything not declared is rejected rather than stored, so a surface cannot
 * smuggle keys through to the runtime.
 */
export interface AnswerRequest {
  /**
   * One entry per declared field. A `secret` FIELD CARRIES `{"provided": true}`
   * AND NOTHING ELSE — the value itself went to the sink. A raw value here is
   * `422 answer_validation_failed`.
   */
  values: Record<string, unknown>;
  /**
   * Which delivery the human answered through. This is what grades that
   * delivery to `acted` — the strongest evidence tier, and the only one that
   * proves a person acted rather than that a transport accepted something.
   */
  via_delivery_id?: DeliveryId | null;
  /**
   * `true` advances a multi-step ask: the server validates, routes secrets to
   * the sink, amends the field set to the next step, re-arms the attempt clock
   * FRESH, appends a step to the eventual receipt, and does NOT signal the
   * waiter. Default `false`.
   */
  partial?: boolean;
  /** The human's own words, recorded verbatim in the receipt. */
  note?: string;
  /**
   * `decide` (the default) settles the request and mints an authorization.
   * `delegate` hands it to `delegate_to` and leaves it `pending`. `unable`
   * records that this person cannot answer.
   */
  disposition?: Disposition;
  delegate_to?: Target;
  /**
   * Which capability sessions this human used to be able to answer. Recorded
   * in the receipt as held time, input counts, and navigated origins —
   * presence and effect, NEVER keystrokes or input payloads. Max 16 items.
   */
  capability_uses?: CapabilityUse[];
  /**
   * Digest of exactly what this human was shown. Required when the request
   * declares `presentation_binding: strict`; a mismatch is
   * `409 presentation_stale`.
   */
  rendered_digest?: Digest;
}

/** One capability session the answerer held while answering. */
export interface CapabilityUse {
  handle: GrantHandle;
  session_ref: GrantSessionRef;
}

/**
 * What the settling write produced. The receipt exists the instant this
 * returns — it is minted in the same transaction as the state change, not
 * written afterwards by a listener that might not run.
 */
export interface AnswerResult {
  /** The settled request, in brief. */
  request: {
    id: RequestId;
    state: RequestState;
    answered_at?: string | null;
  };
  /** The receipt, in brief. `digest` is the chain entry. */
  receipt: {
    id: ReceiptId;
    digest: Digest;
  };
  /**
   * Null for a `partial` answer and for any disposition other than `decide` —
   * there is nothing to spend until a decision has actually been made.
   */
  authorization?: {
    id: AuthorizationId;
    single_use: boolean;
    expires_at?: string | null;
  } | null;
}

// --------------------------------------------------------------- delivery

/**
 * One tracked attempt to reach one target on one channel. Delivery is a
 * first-class entity, not a side effect of a sweep: "we tried" is a claim that
 * has to survive being questioned.
 */
export interface Delivery {
  id: DeliveryId;
  request_id: RequestId;
  /** Open vocabulary. The core carries the name and looks up an adapter. */
  channel: string;
  target?: Target;
  /** Which ladder rung minted this delivery. Rung 0 fires at raise time. */
  rung?: number;
  /**
   * `suppressed` is a real outcome, not a failure — quiet hours, dedupe, or
   * missing consent. `stale` means the request settled through some other
   * delivery. Neither is an error, and both must be visible.
   */
  state:
    | "queued"
    | "suppressed"
    | "sending"
    | "dispatched"
    | "retrying"
    | "failed"
    | "delivered"
    | "bounced"
    | "seen"
    | "acted"
    | "stale"
    | "cancelled";
  /**
   * The strongest evidence this delivery achieved. `dispatched` means our
   * transport accepted it — IT IS NOT EVIDENCE A PERSON GOT ANYTHING.
   */
  grade_reached?: DeliveryGrade | null;
  /**
   * The best grade this channel can ever prove, declared by its adapter. A
   * voice page that cannot authenticate a person tops out at `delivered`,
   * which stops anyone from treating a phone call as consent.
   */
  max_grade?: DeliveryGrade;
  /**
   * Whether this channel can establish *who* received it. A channel that
   * cannot, cannot carry an answer.
   */
  can_authenticate_person?: boolean;
  /** Every send attempt, with exponential backoff and jitter between them. */
  attempts?: DeliveryAttempt[];
  created_at?: string;
  updated_at?: string;
}

/** One transport-level send. */
export interface DeliveryAttempt {
  /** Attempt number, from 1. */
  n: number;
  started_at: string;
  ended_at?: string | null;
  /**
   * What happened at the transport level, independent of whether a person ever
   * saw it.
   */
  outcome?:
    | "accepted"
    | "transient_failure"
    | "permanent_failure"
    | "timeout"
    | "suppressed";
  /** The channel's own status string, carried verbatim for debugging. */
  transport_status?: string | null;
  /** Failure detail. Never contains message content. */
  error?: string | null;
}

/** All deliveries for one request, in creation order. */
export interface DeliveryList {
  data: Delivery[];
}

/**
 * Acknowledgement that another attempt is queued. It is not a claim that
 * anyone was reached.
 */
export interface RedeliverResult {
  delivery_id: DeliveryId;
  queued_at: string;
}

// ----------------------------------------------------------------- waiter

/**
 * One queued notification to a waiter. Signals are a QUEUE, not a flag: an
 * `attempt_lapsed` nudge can never overwrite a subsequent `answered`. READING
 * A SIGNAL DOES NOT CONSUME IT; the ack does.
 */
export interface Signal {
  id: SignalId;
  request_id: RequestId;
  waiter_ref: WaiterRef;
  /**
   * Four terminal types plus `attempt_lapsed`, which is a NON-TERMINAL NUDGE:
   * the request stays `pending` and stays answerable. It fires exactly once,
   * ever, per attempt.
   */
  type: "answered" | "expired" | "cancelled" | "superseded" | "attempt_lapsed";
  /**
   * Monotonically increasing per `waiter_ref`. A client that tracks the
   * highest sequence it has applied can detect gaps and reordering without
   * inspecting content.
   */
  sequence: number;
  resume_token: ResumeToken;
  /** The typed decision. NULL FOR `attempt_lapsed`, which decides nothing. */
  decision?: Decision | null;
  /** Level 2. Returned byte-identical to what was raised. */
  resume_ref?: string | null;
  /** Level 2. Returned byte-identical to what was raised, and in no log. */
  resume_payload?: string | null;
  /**
   * How many times this signal has been pushed to a callback. Redelivery stops
   * at the ack, not at a 2xx.
   */
  attempts?: number;
  created_at: string;
  acked_at?: string | null;
}

/**
 * The typed outcome the runtime consumes. It is DATA THE RUNTIME READS, NEVER
 * AN INSTRUCTION IT MUST OBEY — that distinction is what keeps a decision
 * auditable instead of persuasive.
 */
export interface Decision {
  outcome: "answered" | "expired" | "cancelled" | "superseded";
  /** The answer, keyed by field name. Never carries a `secret` value. */
  values?: Record<string, unknown>;
  /**
   * `human` — a person decided. `policy` — an expiry policy decided.
   * `runtime_inference` — a runtime concluded the human acted. THE PROTOCOL
   * NEVER FABRICATES A PERSON.
   */
  source: "human" | "policy" | "runtime_inference";
  /**
   * For an expiry — whether the effective answer is a denial or the
   * pre-declared default.
   */
  effective?: "deny" | "default" | null;
  receipt_id?: ReceiptId;
  authorization_id?: AuthorizationId | null;
  superseded_by?: RequestId | null;
}

/** Unacked signals for one waiter, oldest first. */
export interface SignalList {
  data: Signal[];
  has_more: boolean;
}

/**
 * Everything a restarted client needs to carry on: the waiter's state, the
 * requests still open under it, and every unacked signal. Nothing was lost
 * while the client was gone.
 */
export interface ReattachResult {
  waiter_ref: WaiterRef;
  state: WaiterState;
  /** Requests under this waiter that are still `pending`. */
  open_requests: RequestId[];
  signals: Signal[];
}

/** Consume a signal. */
export interface AckRequest {
  resume_token: ResumeToken;
  /**
   * Whether the runtime actually applied the decision. `false` IS NOT AN
   * ERROR — it records that the decision arrived and could not be acted on,
   * which is a fact the audit should hold rather than swallow.
   */
  applied: boolean;
  /** Required in practice when `applied` is false. */
  reason?: string;
}

/**
 * Idempotent acknowledgement. `first_ack` distinguishes the write from the
 * replay.
 */
export interface AckResult {
  acked_at: string;
  first_ack: boolean;
}

/** One outbound callback attempt for a signal. */
export interface CallbackAttempt {
  n: number;
  started_at: string;
  ended_at?: string | null;
  /**
   * HTTP status the receiver returned. A 2xx marks the callback dispatched —
   * not consumed.
   */
  status_code?: number | null;
  duration_ms?: number | null;
  outcome?: "accepted" | "transient_failure" | "permanent_failure" | "timeout";
  error?: string | null;
}

/** The attempt log for one signal, oldest first. */
export interface CallbackAttemptList {
  data: CallbackAttempt[];
}

// ---------------------------------------------------------------- receipt

/**
 * An immutable record, minted in the same transaction as the decision, that
 * answers six questions: what was decided, who decided it, when, WHAT THEY
 * SAW, through what, and under what authority. There is no update path —
 * corrections are new receipts.
 */
export interface Receipt {
  id: ReceiptId;
  request_id: RequestId;
  org_id: OrgId;
  /**
   * `decision` — a person decided. `policy` — an expiry policy decided, and
   * the receipt says so plainly. `correction` — this receipt amends the one
   * named in `corrects`; the original stays exactly as it was.
   */
  kind: "decision" | "policy" | "correction";
  corrects?: ReceiptId | null;
  decision: ReceiptDecision;
  actor: ReceiptActor;
  /** Server clock at commit. Never a client-supplied time. */
  decided_at: string;
  /** Which attempt window the decision landed in. */
  attempt_id?: string | null;
  /** The version of the request that was decided on. */
  request_version?: number;
  request_digest?: Digest;
  rendered?: ReceiptRendered;
  via?: ReceiptVia;
  authority?: ReceiptAuthority;
  /**
   * The progressive-disclosure ladder as one intervention rather than as a
   * series of unrelated asks.
   */
  steps?: ReceiptStep[];
  capabilities_exercised?: CapabilityExercised[];
  clearance?: Clearance;
  /**
   * Records that an answer arrived after the requester was already gone. A
   * durable waiter defaults to `keep` precisely so that this answer is still
   * worth recording.
   */
  waiter_state_at_decision?: "armed" | "signalled" | "orphaned";
  chain?: ChainLink;
}

/**
 * What was decided. `secret` FIELDS APPEAR ONLY AS `{"provided": true}` — the
 * receipt records that a credential was supplied, never what it was.
 */
export interface ReceiptDecision {
  values?: Record<string, unknown>;
  disposition?: Disposition;
  /** The human's own words, verbatim. */
  note?: string | null;
}

/**
 * Who decided, with the attestation to back it. `policy` and `runtime` are
 * first-class actor types precisely so that a machine outcome can never be
 * mistaken for consent — a record that cannot distinguish a person from a
 * policy from a passer-by is not a receipt.
 */
export interface ReceiptActor {
  type: "user" | "policy" | "runtime" | "anonymous_link";
  principal_id?: PrincipalId | null;
  /**
   * Display name frozen at decision time, so a later rename does not rewrite
   * history.
   */
  display?: string | null;
  /** The role that justified the decision, frozen at the moment it was made. */
  role_at_decision?: string | null;
  auth_strength?: AuthStrength | null;
  reauth_at?: string | null;
  mfa_at?: string | null;
  /**
   * Salted digest, not an address — enough to correlate a disputed session,
   * not a surveillance record.
   */
  ip_digest?: Digest | null;
  /** Salted digest, for the same reason. */
  user_agent_digest?: Digest | null;
  /**
   * Set when a service account acted for a named person, so delegation is
   * visible rather than collapsed.
   */
  on_behalf_of?: PrincipalId | null;
}

/**
 * What this person actually saw. A digest plus a retained copy, not a
 * re-derivation from the current request — this is what converts "we have a
 * log" into "the log cannot be quietly rewritten to say something else was
 * approved".
 */
export interface ReceiptRendered {
  digest?: Digest;
  /** Opaque pointer to the retained render. Never a public URL. */
  ref?: string | null;
}

/** Through which delivery the decision arrived, and how strong that evidence is. */
export interface ReceiptVia {
  delivery_id?: DeliveryId | null;
  channel?: string | null;
  target?: Target | null;
  grade_reached?: DeliveryGrade | null;
}

/**
 * What the request demanded and what the answerer actually presented. Both, so
 * the two can be compared later.
 */
export interface ReceiptAuthority {
  required?: Authority;
  /**
   * The strength actually established. `none` is what a policy receipt
   * records — honestly.
   */
  satisfied?: "none" | AuthStrength;
}

/**
 * One rung of a progressive-disclosure ladder. Names which fields were
 * provided, never their values.
 */
export interface ReceiptStep {
  n: number;
  at: string;
  /** Field names only. */
  fields_provided?: string[];
  /**
   * Which of those were `secret`. Names only — the values went to the sink and
   * were never here.
   */
  secret_fields?: string[];
  via_delivery_id?: DeliveryId | null;
}

/**
 * Presence and effect, never content. A person driving a live surface types
 * real passwords into it, so a keystroke log would recreate in the audit trail
 * exactly the exposure the whole secret design exists to prevent.
 */
export interface CapabilityExercised {
  handle: GrantHandle;
  session_ref: GrantSessionRef;
  scopes?: GrantScope[];
  resolved_at?: string;
  released_at?: string | null;
  /**
   * Derived from the lease record, so it reflects real presence rather than an
   * optimistic claim.
   */
  held_ms?: number;
  /** A COUNT. No payloads, ever. */
  input_events?: number;
  /** Ordered top-level ORIGINS visited. Origin-level by default. */
  navigations?: string[];
  blast_radius_digest?: Digest;
}

/**
 * How the system knows the human finished. A runtime may *infer* completion,
 * but inference is recorded as inference — `runtime_inference` with no actor —
 * never laundered into a human fact.
 */
export interface Clearance {
  source: "human_assertion" | "runtime_inference" | "timeout";
  /**
   * Null for anything but `human_assertion`. There is no principal to name.
   */
  actor?: PrincipalId | null;
  at?: string;
}

/**
 * This receipt's position in the org's hash chain. Re-walking the chain from a
 * previously exported head detects any rewrite, with no key management at all.
 */
export interface ChainLink {
  height: number;
  /** Null only for the first receipt in an org. */
  prev_digest?: Digest | null;
  digest: Digest;
}

/** One page of receipts. */
export interface ReceiptList {
  data: Receipt[];
  has_more: boolean;
  next_cursor?: string | null;
}

/** The tamper-evidence anchor an external verifier records and later re-checks. */
export interface ChainHead {
  org_id: OrgId;
  /** Number of receipts in the chain. It only ever increases. */
  height: number;
  head_digest: Digest;
  as_of: string;
}

// ---------------------------------------------------------- authorization

/**
 * What the agent spends. The receipt records what was decided; this is the
 * thing that makes the decision usable exactly once. Redemption is idempotent
 * per `effect_key`, so a retried agent turn cannot double-spend.
 */
export interface Authorization {
  id: AuthorizationId;
  receipt_id: ReceiptId;
  request_id: RequestId;
  /** The decided values this authorization carries — what was approved. */
  grants?: Record<string, unknown>;
  /** When true, a second DIFFERENT `effect_key` is `409 authorization_spent`. */
  single_use: boolean;
  /**
   * Defaults to 24 hours. An approval is a decision about a moment; spending
   * it days later is a different act, and the protocol says so.
   */
  expires_at?: string | null;
  /** What this authorization is tied to. */
  bound_to?: {
    waiter_ref?: WaiterRef | null;
    /**
     * Binds the authorization to the SHAPE of the effect. An approval of
     * "refund $2,400" cannot be spent on "refund $24,000" — the mismatch is
     * `409 effect_digest_mismatch`.
     */
    effect_digest?: Digest | null;
  };
  redemptions?: Redemption[];
  state: "open" | "spent" | "expired";
}

/** One spend, keyed by the caller's own effect key. */
export interface Redemption {
  effect_key: string;
  redeemed_at: string;
}

/** Spend the authorization against exactly one effect. */
export interface RedeemRequest {
  /**
   * A stable identifier for the effect this decision authorizes — the same
   * string on every retry of the same effect. Choosing a key that varies per
   * attempt defeats the entire mechanism. 1–256 characters.
   */
  effect_key: string;
  /**
   * Digest of the effect's parameters, compared against
   * `bound_to.effect_digest` when that is set.
   */
  effect_digest?: Digest;
}

/**
 * `first_redemption` is the whole answer a caller needs: `true` means act,
 * `false` means this effect already happened and must not happen again.
 */
export interface RedeemResult {
  redeemed_at: string;
  first_redemption: boolean;
}

// ------------------------------------------------------------------- sink

/**
 * Secret values, on their way to a runtime-owned sink. Never logged, echoed,
 * or placed in a URL.
 */
export interface SinkValuesRequest {
  /**
   * Keys MUST be declared `requires.answer.fields` names on the owning
   * request. Undeclared keys are rejected outright — the allowlist is what
   * stops a surface from smuggling arbitrary keys through to the runtime.
   */
  values: Record<string, unknown>;
}

/** Names only. A response that echoed a value would defeat the point. */
export interface SinkValuesResult {
  /** Which declared field names the sink took. */
  accepted: string[];
  /** The sink's own opaque progress label. */
  state?: string | null;
}

// ------------------------------------------------------------------- meta

/**
 * What this deployment supports. A conformance runner reads this first so it
 * can distinguish "not implemented" from "failed", and a client reads it to
 * learn whether a declaration it wants to make will fail closed before it
 * makes it.
 */
export interface Meta {
  /** The wire contract version this server implements. */
  protocol_version: "0.1";
  /**
   * `1` — the full protocol without continuation state. `2` — additionally
   * returns `resume_ref` and `resume_payload` byte-identically in signals, and
   * stores them nowhere else.
   */
  conformance_level: 1 | 2;
  /** Named optional behaviours, for example `continuation`. */
  extensions?: string[];
  /**
   * Answer field types this server will accept. Anything outside this list
   * fails closed with `400 unsupported_field_type`.
   */
  field_types?: string[];
  /**
   * Capability types this server will accept. Anything outside this list fails
   * closed with `400 unsupported_capability_type`.
   */
  capability_types?: string[];
  /**
   * The largest long-poll window this server will honour. Larger values are
   * clamped, not rejected.
   */
  max_wait_seconds: 30;
  [key: string]: unknown;
}

// ----------------------------------------------------------------- errors

/**
 * The complete, stable error taxonomy. `code` is machine-readable and does not
 * change within a major version; `message` is for humans and may change at any
 * time. Clients branch on `code`, never on `message` and never on the HTTP
 * status alone.
 */
export type ErrorCode =
  | "invalid_request"
  | "unsupported_field_type"
  | "unsupported_capability_type"
  | "unsupported_requires_version"
  | "invalid_api_key"
  | "authentication_required"
  | "insufficient_scope"
  | "product_not_entitled"
  | "insufficient_authority"
  | "requester_may_not_answer"
  | "tenant_mismatch"
  | "auth_strength_not_permitted"
  | "request_not_found"
  | "capability_not_found"
  | "signal_not_found"
  | "authorization_not_found"
  | "already_answered"
  | "request_expired"
  | "request_cancelled"
  | "request_superseded"
  | "request_in_progress"
  | "idempotency_key_reused"
  | "authorization_spent"
  | "authorization_expired"
  | "effect_digest_mismatch"
  | "blast_radius_mismatch"
  | "grant_already_held"
  | "presentation_stale"
  | "capability_expired"
  | "answer_validation_failed"
  | "rate_limited"
  | "delivery_unavailable";

/**
 * One field-level validation failure, so a surface can highlight the input in
 * place.
 */
export interface FieldError {
  /** The declared field name that failed. */
  name: string;
  /**
   * Stable machine-readable reason — `required`, `not_an_option`,
   * `out_of_range`, `secret_value_not_permitted`.
   */
  code: string;
  message?: string;
}

/**
 * The error itself. Extra keys are context for the specific code, and are
 * absent otherwise.
 */
export interface ErrorBody {
  code: ErrorCode;
  /** Human-readable and subject to change. Never parse it. */
  message: string;
  /** Present on `already_answered`, `delivery_unavailable`, and friends. */
  request_id?: RequestId | null;
  /** Present on `already_answered` — the decision that already exists. */
  receipt_id?: ReceiptId | null;
  /** Present on `request_superseded` — where to send the human instead. */
  superseded_by?: RequestId | null;
  /** Present on `answer_validation_failed`. */
  fields?: FieldError[];
  /** A stable link explaining this code and what to do about it. */
  docs?: string;
}

/** One envelope for every error in the protocol. There is never a second shape. */
export interface Error {
  error: ErrorBody;
}

// #endregion openapi:components/schemas

// ---------------------------------------------------------------------------
// Per-operation aliases, derived from `paths:`. Named after each operation's
// `operationId`. `…Body` is the request body; `…Response` is the success
// response. Every operation may additionally return `Error` (see `ErrorCode`).
// These are NOT schemas and the drift check ignores them.
// ---------------------------------------------------------------------------

/**
 * `Idempotency-Key` header. Required on `POST /requests`, strongly recommended
 * on every other mutating call. Same key + same body digest replays the stored
 * representation; same key + a different body digest is
 * `409 idempotency_key_reused`. 1–255 characters.
 */
export type IdempotencyKey = string;

/**
 * Long-poll window in seconds. `0` (or omitted) returns immediately. Clamped
 * to `Meta.max_wait_seconds`, which is 30.
 */
export type WaitSeconds = number;

/** Opaque forward cursor — pass `next_cursor` from the previous page. */
export type Cursor = string;

/** `POST /requests` — raiseRequest. */
export type RaiseRequestBody = RaiseRequest;
/** `201` on create, `200` on an idempotent replay. */
export type RaiseRequestResponse = Request;

/** `GET /requests` — listRequests query parameters. */
export interface ListRequestsQuery {
  /** Repeat the parameter to match more than one state. */
  state?: RequestState | RequestState[];
  /** Matched byte-for-byte; the server never parses the value. */
  waiter_ref?: string;
  /** Filter to requests a given principal is a delivery target of. */
  assignee?: string;
  /** Filter to requests with at least one delivery on this channel. */
  channel?: string;
  /** Inclusive lower bound on `created_at`. */
  created_after?: string;
  /** Exclusive upper bound on `created_at`. */
  created_before?: string;
  /** 1–200, default 50. */
  limit?: number;
  cursor?: Cursor;
}
/** `GET /requests` — listRequests. */
export type ListRequestsResponse = RequestList;

/** `GET /requests/{request_id}` — getRequest query parameters. */
export interface GetRequestQuery {
  wait?: WaitSeconds;
}
/** `GET /requests/{request_id}` — getRequest. */
export type GetRequestResponse = Request;

/** `POST /requests/{request_id}/amend` — amendRequest. */
export type AmendRequestBody = AmendRequest;
/** The amended request, with `version` incremented. */
export type AmendRequestResponse = Request;

/** `POST /requests/{request_id}/cancel` — cancelRequest. */
export type CancelRequestBody = CancelRequest;
/** The cancelled request. */
export type CancelRequestResponse = Request;

/** `POST /requests/{request_id}/supersede` — supersedeRequest. */
export type SupersedeRequestBody = SupersedeRequest;
/** The superseded request, carrying `superseded_by`. */
export type SupersedeRequestResponse = Request;

/** `POST /requests/{request_id}/escalate` — escalateRequest. Body is optional. */
export type EscalateRequestBody = EscalateRequest;
/** The request, with the new rung's deliveries attached. */
export type EscalateRequestResponse = Request;

/** `POST /requests/{request_id}/reassign` — reassignRequest. */
export type ReassignRequestBody = ReassignRequest;
/** The request, retargeted. */
export type ReassignRequestResponse = Request;

/** `POST /requests/{request_id}/attempt` — armAttempt. Body is optional. */
export type ArmAttemptBody = AttemptRequest;
/** The request, with `attempt_expires_at` set. */
export type ArmAttemptResponse = Request;

/** `POST /requests/{request_id}/answer` — answerRequest. Human principals only. */
export type AnswerRequestBody = AnswerRequest;
/** The settled request, its receipt, and (unless partial) its authorization. */
export type AnswerRequestResponse = AnswerResult;

/** `GET /requests/{request_id}/receipt` — getRequestReceipt. */
export type GetRequestReceiptResponse = Receipt;

/** `GET /requests/{request_id}/deliveries` — listRequestDeliveries. */
export type ListRequestDeliveriesResponse = DeliveryList;

/** `GET /receipts` — listReceipts query parameters. */
export interface ListReceiptsQuery {
  /** Inclusive lower bound on `decided_at`. */
  created_after?: string;
  /** Exclusive upper bound on `decided_at`. */
  created_before?: string;
  /** 1–200, default 50. */
  limit?: number;
  cursor?: Cursor;
}
/** `GET /receipts` — listReceipts. */
export type ListReceiptsResponse = ReceiptList;

/** `GET /receipts/chain-head` — getReceiptChainHead. */
export type GetReceiptChainHeadResponse = ChainHead;

/** `GET /receipts/{receipt_id}` — getReceipt. */
export type GetReceiptResponse = Receipt;

/** `GET /waiters/{waiter_ref}/signals` — pollWaiterSignals query parameters. */
export interface PollWaiterSignalsQuery {
  wait?: WaitSeconds;
}
/**
 * `GET /waiters/{waiter_ref}/signals` — pollWaiterSignals. The server returns
 * `204` with NO BODY when the long-poll window closes with nothing pending.
 */
export type PollWaiterSignalsResponse = SignalList;

/** `POST /waiters/{waiter_ref}/reattach` — reattachWaiter. No request body. */
export type ReattachWaiterResponse = ReattachResult;

/** `POST /signals/{signal_id}/ack` — ackSignal. */
export type AckSignalBody = AckRequest;
/** `POST /signals/{signal_id}/ack` — ackSignal. */
export type AckSignalResponse = AckResult;

/** `GET /signals/{signal_id}/attempts` — listSignalAttempts. */
export type ListSignalAttemptsResponse = CallbackAttemptList;

/** `GET /authorizations/{authorization_id}` — getAuthorization. */
export type GetAuthorizationResponse = Authorization;

/** `POST /authorizations/{authorization_id}/redeem` — redeemAuthorization. */
export type RedeemAuthorizationBody = RedeemRequest;
/** `POST /authorizations/{authorization_id}/redeem` — redeemAuthorization. */
export type RedeemAuthorizationResponse = RedeemResult;

/** `GET /grants/{handle}` — getCapabilityGrant. Human session only. */
export type GetCapabilityGrantResponse = CapabilityGrant;

/** `DELETE /grants/{handle}` — revokeCapabilityGrant. Body is optional. */
export type RevokeCapabilityGrantBody = RevokeGrantRequest;
/** `204` with no body. Repeating the call is a no-op and also returns `204`. */
export type RevokeCapabilityGrantResponse = void;

/** `POST /grants/{handle}/sessions` — resolveCapabilityGrant. */
export type ResolveCapabilityGrantBody = CreateGrantSessionRequest;
/** A live session. Renew before `lease_until` or the session closes. */
export type ResolveCapabilityGrantResponse = GrantSession;

/** `POST /grants/{handle}/sessions/{session_ref}/renew` — renewGrantSession. */
export type RenewGrantSessionResponse = RenewResult;

/** `DELETE /grants/{handle}/sessions/{session_ref}` — releaseGrantSession. */
export type ReleaseGrantSessionResponse = void;

/** `POST /sinks/{sink_ref}/values` — submitSinkValues. Human session only. */
export type SubmitSinkValuesBody = SinkValuesRequest;
/** `202`. Names only, never values. */
export type SubmitSinkValuesResponse = SinkValuesResult;

/** `GET /deliveries/{delivery_id}` — getDelivery. */
export type GetDeliveryResponse = Delivery;

/** `POST /deliveries/{delivery_id}/redeliver` — redeliverDelivery. No body. */
export type RedeliverDeliveryResponse = RedeliverResult;

/** `GET /meta` — getMeta. Unauthenticated. */
export type GetMetaResponse = Meta;

/** Every non-2xx response in the protocol uses this one envelope. */
export type ErrorResponse = Error;
