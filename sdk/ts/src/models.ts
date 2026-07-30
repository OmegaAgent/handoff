/**
 * Typed views over the protocol's objects, and the builders that declare a request.
 *
 * Nothing here switches on an interaction type. A request declares *what it needs* — the shape of
 * the answer, the capabilities the person must be handed, the authority required to give it — and
 * every one of the eight interaction patterns in §5.6 is a different declaration rather than a
 * different code path (I14). The builders below construct declarations; they are constructors, not
 * kinds, and the server never sees which one you called.
 */

import { compact, Doc, type JsonObject } from "./document.ts";

/** §8.3. `attempt_lapsed` is deliberately absent: it is a nudge, and the request stays pending. */
export const TERMINAL_SIGNAL_TYPES = ["answered", "expired", "cancelled", "superseded"] as const;

export type SignalType = (typeof TERMINAL_SIGNAL_TYPES)[number] | "attempt_lapsed";
export type DecisionSource = "human" | "policy" | "runtime_inference";
export type ActorType = "user" | "policy" | "runtime" | "anonymous_link";
export type RequestState = "pending" | "answered" | "expired" | "cancelled" | "superseded";
export type AuthStrength = "link_only" | "session" | "reauth" | "mfa";
export type Disposition = "decide" | "delegate" | "unable";
export type OnExpiry = "escalate" | "expire_and_deny" | "default" | "park";

export type FieldOption = { id: string; label: string };

// -- declaration builders ---------------------------------------------------------------------

/** Constructors for the declared answer fields of §5.3. Metadata only: they declare the shape of
 *  an answer and never carry one. */
export const fields = {
  choice(
    name: string,
    label: string,
    options: Array<string | FieldOption | [string, string]>,
    opts: { required?: boolean; multi?: boolean } = {},
  ): JsonObject {
    const normalized = options.map((option) =>
      typeof option === "string"
        ? { id: option, label: option }
        : Array.isArray(option)
          ? { id: option[0], label: option[1] }
          : { id: option.id, label: option.label ?? option.id },
    );
    return compact({
      name,
      label,
      type: "choice",
      required: opts.required ?? true,
      options: normalized,
      multi: opts.multi,
    });
  },

  text(name: string, label: string, opts: { required?: boolean; maxLen?: number } = {}): JsonObject {
    return compact({
      name,
      label,
      type: "text",
      required: opts.required ?? true,
      max_len: opts.maxLen,
    });
  },

  number(name: string, label: string, opts: { required?: boolean } = {}): JsonObject {
    return compact({ name, label, type: "number", required: opts.required ?? true });
  },

  boolean(name: string, label: string, opts: { required?: boolean } = {}): JsonObject {
    return compact({ name, label, type: "boolean", required: opts.required ?? true });
  },

  /**
   * A value the person types that the agent must never see (§12).
   *
   * Declaring one raises the effective authority floor server-side to the administrative role
   * (§4.3), and the answer carries `{"provided": true}` and nothing else. The value itself travels
   * to the sink, which the runtime owns; this protocol carries the declaration, never the
   * credential.
   */
  secret(name: string, label: string, opts: { required?: boolean; sinkRef?: string } = {}): JsonObject {
    return compact({
      name,
      label,
      type: "secret",
      required: opts.required ?? true,
      sink_ref: opts.sinkRef,
    });
  },

  attestation(name: string, label: string, opts: { required?: boolean } = {}): JsonObject {
    return compact({ name, label, type: "attestation", required: opts.required ?? true });
  },

  document(
    name: string,
    label: string,
    opts: { required?: boolean; schemaRef?: string; initial?: unknown } = {},
  ): JsonObject {
    return compact({
      name,
      label,
      type: "document",
      required: opts.required ?? true,
      schema_ref: opts.schemaRef,
      initial: opts.initial,
    });
  },

  fileRef(name: string, label: string, opts: { required?: boolean } = {}): JsonObject {
    return compact({ name, label, type: "file_ref", required: opts.required ?? true });
  },
};

/** What the person is shown alongside the ask, so the receipt can bind to it. */
export const evidence = {
  link(label: string, url: string): JsonObject {
    return { kind: "link", label, url };
  },
  table(label: string, columns: string[], rows: unknown[][]): JsonObject {
    return { kind: "table", label, value: { columns, rows } };
  },
  text(label: string, value: string): JsonObject {
    return { kind: "text", label, value };
  },
};

/** What the person reads (§5.2). */
export function prompt(title: string, body?: string, evidenceItems?: JsonObject[]): JsonObject {
  return compact({ title, body, evidence: evidenceItems?.length ? evidenceItems : undefined });
}

/**
 * Who is entitled to answer, evaluated at answer time against the answerer (§4.3).
 *
 * `forbid_requester` is always true and is emitted for clarity; a server rejects any other value.
 * A machine cannot answer its own request under any configuration (§4.2, I15).
 */
export function authority(
  minRole = "editor",
  authStrength: AuthStrength = "session",
  opts: { assignees?: JsonObject[]; quorum?: number; reason?: string } = {},
): JsonObject {
  return compact({
    min_role: minRole,
    auth_strength: authStrength,
    assignees: opts.assignees,
    quorum: opts.quorum,
    forbid_requester: true,
    reason: opts.reason,
  });
}

/**
 * Declare something the person must be handed in order to be able to answer (§5.4, §11).
 *
 * A capability is carried as an opaque handle. Nothing resolvable — no URL, no token, no
 * credential — travels in a declaration, a receipt, a signal, or a delivery (§11.1, I8). The
 * person's own client exchanges the handle for a session; the agent runtime cannot.
 */
export function capability(
  type: string,
  opts: {
    scope?: "view" | "drive";
    handle?: string;
    provider?: string;
    resourceRef?: string;
    optional?: boolean;
    ttl?: string;
    label?: string;
    purpose?: string;
    constraints?: JsonObject;
  } = {},
): JsonObject {
  return compact({
    handle: opts.handle,
    type,
    scope: opts.scope ?? "view",
    provider: opts.provider,
    resource_ref: opts.resourceRef,
    optional: opts.optional,
    ttl: opts.ttl,
    label: opts.label,
    purpose: opts.purpose,
    constraints: opts.constraints,
  });
}

/**
 * The versioned declaration envelope (§5.2).
 *
 * An empty field list is legitimate and means the whole request is an attestation: there is
 * nothing to type and the person acts out of band.
 */
export function requires(
  answerFields: JsonObject[] = [],
  opts: { capabilities?: JsonObject[]; authority?: JsonObject; valueSink?: JsonObject; v?: number } = {},
): JsonObject {
  return compact({
    v: opts.v ?? 1,
    answer: compact({ fields: answerFields, value_sink: opts.valueSink }),
    capabilities: opts.capabilities ?? [],
    authority: opts.authority,
  });
}

/**
 * What happens when nobody answers (§6.4).
 *
 * `default` is the only policy that produces an outcome without a person, so the default answer
 * must be declared here, at raise time, before anyone knew the person would go quiet. The
 * resulting receipt records `actor.type = "policy"` and no audit can mistake it for consent.
 */
export function ttlPolicy(
  onExpiry: OnExpiry = "expire_and_deny",
  opts: { defaultAnswer?: JsonObject; reminderEvery?: string } = {},
): JsonObject {
  if (onExpiry === "default" && opts.defaultAnswer === undefined) {
    throw new Error(
      'ttlPolicy("default") requires defaultAnswer: the pre-agreed answer must be declared at ' +
        "raise time (§6.4)",
    );
  }
  return compact({
    on_expiry: onExpiry,
    default_answer: opts.defaultAnswer,
    reminder_every: opts.reminderEvery,
  });
}

/** Seconds to an ISO 8601 duration (§1.4). */
export function isoDuration(seconds: number | undefined): string | undefined {
  return seconds === undefined ? undefined : `PT${Math.floor(seconds)}S`;
}

// -- typed views ------------------------------------------------------------------------------

/**
 * The typed outcome a runtime consumes.
 *
 * Data the runtime reads, never an instruction it must obey. `values` never carries a secret: a
 * `secret` field is reduced to `{"provided": true}` before anything leaves the sink (§12, I7).
 */
export class Decision extends Doc {
  get outcome(): string {
    return this.data.outcome;
  }
  get values(): JsonObject {
    return this.data.values ?? {};
  }
  /** `human`, `policy`, or `runtime_inference`. The protocol never fabricates a person. */
  get source(): DecisionSource {
    return this.data.source;
  }
  get decidedByHuman(): boolean {
    return this.data.source === "human";
  }
  get effective(): string | null {
    return this.data.effective ?? null;
  }
  get receiptId(): string | null {
    return this.data.receipt_id ?? null;
  }
  get authorizationId(): string | null {
    return this.data.authorization_id ?? null;
  }
  get supersededBy(): string | null {
    return this.data.superseded_by ?? null;
  }
}

/**
 * One queued notification to a waiter.
 *
 * Signals are a queue, not a flag, so an `attempt_lapsed` nudge can never overwrite a subsequent
 * terminal signal (§8.2 W2). Reading a signal does not consume it — consumption is the ack, and
 * that two-step is what turns at-least-once delivery into effectively-once application (§8.3).
 */
export class Signal extends Doc {
  get id(): string {
    return this.data.id;
  }
  get requestId(): string {
    return this.data.request_id;
  }
  get waiterRef(): string {
    return this.data.waiter_ref;
  }
  get type(): SignalType {
    return this.data.type;
  }
  get sequence(): number {
    return this.data.sequence;
  }
  /** Required to ack this signal. Never logged, never rendered into a string. */
  get resumeToken(): string {
    return this.data.resume_token;
  }
  get decision(): Decision | null {
    const raw = this.data.decision;
    return raw && typeof raw === "object" ? new Decision(raw) : null;
  }
  get isTerminal(): boolean {
    return (TERMINAL_SIGNAL_TYPES as readonly string[]).includes(this.data.type);
  }
  get ackedAt(): string | null {
    return this.data.acked_at ?? null;
  }
  /** Level 2. Whatever the runtime stored at raise time, returned byte-identical (§14). */
  get resumeRef(): string | null {
    return this.data.resume_ref ?? null;
  }
  /** Level 2. Opaque bytes the runtime owns; the server stores them and never reads them. */
  get resumePayload(): string | null {
    return this.data.resume_payload ?? null;
  }
}

export class Prompt extends Doc {
  get title(): string {
    return this.data.title;
  }
  get body(): string | undefined {
    return this.data.body;
  }
}

export class Request extends Doc {
  get id(): string {
    return this.data.id;
  }
  get state(): RequestState {
    return this.data.state;
  }
  get waiterRef(): string {
    return this.data.waiter_ref;
  }
  get version(): number {
    return this.data.version ?? 1;
  }
  get isPending(): boolean {
    return this.data.state === "pending";
  }
  /** Where a person goes to answer. A locator, not a capability: opening it prompts for
   *  authentication, and possessing it authorizes nothing (§4.6). */
  get surfaceUrl(): string | undefined {
    return this.data.surface_url;
  }
  get prompt(): Prompt {
    return new Prompt(this.data.prompt ?? {});
  }
}

/** The immutable record of an outcome, minted in the same transaction as the state change. */
export class Receipt extends Doc {
  get id(): string {
    return this.data.id;
  }
  get kind(): string {
    return this.data.kind;
  }
  get decidedAt(): string {
    return this.data.decided_at;
  }
  /** A receipt that cannot say who decided is not a receipt (§4.4, §9.2). */
  get actorType(): ActorType {
    return this.data.actor?.type;
  }
  get decidedByHuman(): boolean {
    return this.actorType === "user";
  }
  get chainDigest(): string | undefined {
    return this.data.chain?.digest;
  }
  /** The receipt without its `chain` member — the byte sequence digests are taken over. */
  core(): JsonObject {
    const out: JsonObject = {};
    for (const key of Object.keys(this.data)) if (key !== "chain") out[key] = this.data[key];
    return out;
  }
}

/** What the runtime spends. One answer mints exactly one (§10, I10). */
export class Authorization extends Doc {
  get id(): string {
    return this.data.id;
  }
  get singleUse(): boolean {
    return this.data.single_use ?? true;
  }
  get expiresAt(): string | null {
    return this.data.expires_at ?? null;
  }
}

export class AnswerResult extends Doc {
  get receiptId(): string {
    return this.data.receipt.id;
  }
  get authorizationId(): string | null {
    return this.data.authorization?.id ?? null;
  }
}

export class AckResult extends Doc {
  /** False on a replay. Both calls return 200; redelivery stops once (§3.5, C-12). */
  get firstAck(): boolean {
    return this.data.first_ack;
  }
  get ackedAt(): string {
    return this.data.acked_at;
  }
}

export class RedeemResult extends Doc {
  /** The whole answer a caller needs: true means act, false means this effect already happened
   *  and must not happen again (§10, C-13). */
  get firstRedemption(): boolean {
    return this.data.first_redemption;
  }
  get redeemedAt(): string {
    return this.data.redeemed_at;
  }
}

/** What a restarted process gets back: the waiter's state, its open requests, and every signal
 *  that is still unacked. Nothing was lost while the client was gone (§8.5). */
export class ReattachResult extends Doc {
  get waiterRef(): string {
    return this.data.waiter_ref;
  }
  get state(): string {
    return this.data.state;
  }
  get openRequests(): string[] {
    return this.data.open_requests ?? [];
  }
  get signals(): Signal[] {
    return (this.data.signals ?? []).map((s: JsonObject) => new Signal(s));
  }
}

/** What a deployment supports. Read it to learn that a declaration will fail closed before making
 *  it, rather than after (§19). */
export class Meta extends Doc {
  get protocolVersion(): string {
    return this.data.protocol_version;
  }
  get conformanceLevel(): number {
    return this.data.conformance_level;
  }
  get maxWaitSeconds(): number {
    return this.data.max_wait_seconds;
  }
  get fieldTypes(): string[] {
    return this.data.field_types ?? [];
  }
  get capabilityTypes(): string[] {
    return this.data.capability_types ?? [];
  }
  get extensions(): string[] {
    return this.data.extensions ?? [];
  }
  supports(extension: string): boolean {
    return this.extensions.includes(extension);
  }
}
