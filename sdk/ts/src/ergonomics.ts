/**
 * Two shorthands for the two asks people reach for first.
 *
 * Both are ordinary declarations. `ask` declares one `text` field, `approve` declares one `choice`
 * field, and the server cannot tell which function you called — there is no kind on the wire and no
 * branch behind it (I14). Anything they do not cover is `Client.raiseRequest` with a declaration you
 * build yourself, which is the same code path with more of it spelled out.
 */

import type { Client } from "./client.ts";
import { newIdempotencyKey } from "./client.ts";
import type { JsonObject } from "./document.ts";
import { HandoffTimeout } from "./errors.ts";
import { authority, fields, isoDuration, prompt, requires, ttlPolicy } from "./models.ts";
import { Outcome } from "./waiter.ts";

function waiterRefFor(explicit?: string): string {
  return explicit ?? `run:${newIdempotencyKey()}`;
}

export interface AskOptions {
  default?: string;
  body?: string;
  evidence?: JsonObject[];
  /** Milliseconds. */
  timeoutMs?: number;
  ttl?: string;
  waiterRef?: string;
  minRole?: string;
  authStrength?: "link_only" | "session" | "reauth" | "mfa";
  label?: string;
  metadata?: JsonObject;
  ack?: boolean;
}

/**
 * Ask a person a question and block until they answer. Resolves with their text.
 *
 * `default` is declared **at raise time**, not applied locally afterwards. It becomes
 * `ttl_policy: {on_expiry: "default", default_answer: …}`, so when nobody answers the server mints a
 * policy receipt with `actor.type = "policy"` and the record cannot be mistaken for consent (§6.4).
 * Guessing a default in the client after the fact would produce the same value with no record at
 * all — the same behaviour and a worse audit trail.
 *
 * A `default` also needs a `ttl`: without one the request never expires, so the policy never fires.
 * If you give a `timeoutMs` and no `ttl`, the timeout is used as the ttl so the local deadline and
 * the declared one agree.
 *
 * This is the convenient blocking form and it acks on receipt, which means an answer is consumed
 * the moment it reaches this process. Where losing it to a crash between here and your side effect
 * would matter, use `Waiter.receive` instead: it acks after your callback has applied the outcome,
 * and leaves the signal queued if it rejects.
 */
export async function ask(
  client: Client,
  question: string,
  options: AskOptions = {},
): Promise<string | undefined> {
  const timeoutMs = options.timeoutMs ?? 600_000;
  let ttl = options.ttl;
  if (options.default !== undefined && ttl === undefined) ttl = isoDuration(timeoutMs / 1000);

  const pending = await client.raiseRequest({
    waiterRef: waiterRefFor(options.waiterRef),
    prompt: prompt(question, options.body, options.evidence),
    requires: requires([fields.text("answer", options.label ?? "Answer")], {
      authority: authority(options.minRole ?? "viewer", options.authStrength ?? "session"),
    }),
    ttl,
    ttlPolicy:
      options.default !== undefined && ttl !== undefined
        ? ttlPolicy("default", { defaultAnswer: { answer: options.default } })
        : undefined,
    metadata: options.metadata,
  });

  let signal;
  try {
    signal = await pending.wait({ timeoutMs });
  } catch (error) {
    if (error instanceof HandoffTimeout && options.default !== undefined) return options.default;
    throw error;
  }

  if (options.ack !== false) await pending.waiter.ack(signal, { applied: true });
  const outcome = new Outcome(signal, client);
  if (outcome.outcome !== "answered" && !("answer" in outcome.values)) {
    if (options.default !== undefined) return options.default;
    throw new HandoffTimeout(`request ${pending.id} ended as '${outcome.outcome}' with no answer`, {
      waiterRef: pending.waiterRef,
      requestId: pending.id,
    });
  }
  return outcome.value<string>("answer");
}

export interface ApproveOptions {
  body?: string;
  evidence?: JsonObject[];
  approveLabel?: string;
  rejectLabel?: string;
  withNote?: boolean;
  /** Milliseconds. */
  timeoutMs?: number;
  ttl?: string;
  waiterRef?: string;
  minRole?: string;
  authStrength?: "link_only" | "session" | "reauth" | "mfa";
  reason?: string;
  mode?: "advisory" | "gated";
  metadata?: JsonObject;
  ack?: boolean;
}

/**
 * Ask a person to approve or reject, and block. Resolves with an `Outcome`.
 *
 * `outcome.approved` is true only when a person chose to approve. An expiry, a cancellation, and a
 * supersession are all false and all carry their own typed outcome — the request never goes quiet,
 * and "nobody answered" is never silently the same as "approved" (§6.4, I11).
 *
 * Pass `mode: "gated"` when the effect must not happen without a redemption, then call
 * `outcome.redeem(effectKey)` immediately before performing it and act only when `firstRedemption`
 * is true (§10).
 */
export async function approve(
  client: Client,
  title: string,
  options: ApproveOptions = {},
): Promise<Outcome> {
  const declared = [
    fields.choice("decision", "Decision", [
      ["approve", options.approveLabel ?? "Approve"],
      ["reject", options.rejectLabel ?? "Reject"],
    ]),
  ];
  if (options.withNote !== false) {
    declared.push(fields.text("note", "Add a note", { required: false, maxLen: 500 }));
  }

  const pending = await client.raiseRequest({
    waiterRef: waiterRefFor(options.waiterRef),
    prompt: prompt(title, options.body, options.evidence),
    requires: requires(declared, {
      authority: authority(options.minRole ?? "editor", options.authStrength ?? "session", {
        reason: options.reason,
      }),
    }),
    ttl: options.ttl,
    mode: options.mode,
    metadata: options.metadata,
  });

  const signal = await pending.wait({ timeoutMs: options.timeoutMs ?? 600_000 });
  if (options.ack !== false) await pending.waiter.ack(signal, { applied: true });
  const outcome = new Outcome(signal, client);
  return new Outcome(signal, client, outcome.value("decision") === "approve");
}
