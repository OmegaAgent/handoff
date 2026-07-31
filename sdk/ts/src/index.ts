/**
 * The TypeScript client for the Handoff protocol — human intervention in automated work.
 *
 * A program that cannot proceed asks a person, a person answers, and the answer comes back as typed
 * data with a durable record of who decided what, on what basis, and through which channel.
 *
 * ```ts
 * import * as handoff from "@handoffproto/sdk";
 *
 * handoff.configure({ baseUrl: "https://handoff.example.com", apiKey });
 *
 * const address = await handoff.ask("Which shipping address should I use?");
 *
 * const outcome = await handoff.approve("Refund $2,400 to Acme Corp?", { mode: "gated" });
 * if (outcome.approved && (await outcome.redeem("stripe:refund:ch_1B")).firstRedemption) {
 *   await stripe.refund("ch_1B");
 * }
 * ```
 *
 * The wait is not in this process. It is a durable row on the server keyed by `waiterRef` (§8), so
 * a client may die at any point and a later one reattaches and finds the answer still there,
 * unacked:
 *
 * ```ts
 * const waiter = await handoff.resume("run:0198f2a1");
 * await waiter.receive(async (received) => {
 *   await apply(received.values);   // the ack is sent when this resolves
 * });
 * ```
 *
 * What that buys is precise, and it is worth stating precisely: **one human answer, delivered to
 * your agent exactly once, as typed data, authorizing exactly one effect.** It is not execution
 * resumption. Whether your program can pick up where it stopped is a property of your program; this
 * protocol makes sure the answer is there when it does (§1.3).
 */

export const VERSION = "0.2.0";
export const PROTOCOL_VERSION = "0.1";

export { Client, DEFAULT_BASE_URL, newIdempotencyKey } from "./client.ts";
export type { ClientOptions, RaiseOptions } from "./client.ts";

export { Outcome, PendingRequest, Received, Waiter } from "./waiter.ts";
export type { NextOptions } from "./waiter.ts";

export {
  AckResult,
  AnswerResult,
  Authorization,
  Decision,
  Meta,
  Prompt,
  ReattachResult,
  Receipt,
  RedeemResult,
  Request,
  Signal,
  TERMINAL_SIGNAL_TYPES,
  authority,
  capability,
  evidence,
  fields,
  isoDuration,
  prompt,
  requires,
  ttlPolicy,
} from "./models.ts";
export type {
  ActorType,
  AuthStrength,
  DecisionSource,
  Disposition,
  FieldOption,
  OnExpiry,
  RequestState,
  SignalType,
} from "./models.ts";

export {
  Doc,
  canonicalBytes,
  compact,
  constantTimeEquals,
  decodeDocument,
  digest,
  encodeDocument,
  sha256Hex,
  toHex,
} from "./document.ts";
export type { JsonObject, JsonValue } from "./document.ts";

export {
  FRESHNESS_WINDOW_SECONDS,
  SIGNATURE_VERSION,
  callbackCanonicalString,
  chainDigest,
  receiptCoreHash,
  signCallback,
  verifyCallback,
  verifyChain,
  verifyReceiptChain,
} from "./signing.ts";
export type { HeaderBag, VerifiedCallback } from "./signing.ts";

export * from "./errors.ts";

export { ask as askWith, approve as approveWith } from "./ergonomics.ts";
export type { AskOptions, ApproveOptions } from "./ergonomics.ts";

import { Client } from "./client.ts";
import type { ClientOptions, RaiseOptions } from "./client.ts";
import { ask as askWithClient, approve as approveWithClient } from "./ergonomics.ts";
import type { ApproveOptions, AskOptions } from "./ergonomics.ts";
import type { Meta, RedeemResult } from "./models.ts";
import type { Outcome, PendingRequest, Waiter } from "./waiter.ts";

let defaultClient: Client | undefined;

/** Point the module-level helpers at a server. Falls back to `$HANDOFF_URL` and `$HANDOFF_API_KEY`. */
export function configure(options: ClientOptions = {}): Client {
  defaultClient = new Client(options);
  return defaultClient;
}

/** The module-level client, created from the environment on first use. */
export function client(): Client {
  if (!defaultClient) defaultClient = new Client();
  return defaultClient;
}

/** Raise a request on the module-level client. See `Client.raiseRequest`. */
export function raiseRequest(options: RaiseOptions): Promise<PendingRequest> {
  return client().raiseRequest(options);
}

/** Ask a person a question and block until they answer. */
export function ask(question: string, options: AskOptions = {}): Promise<string | undefined> {
  return askWithClient(client(), question, options);
}

/** Ask a person to approve or reject, and block. */
export function approve(title: string, options: ApproveOptions = {}): Promise<Outcome> {
  return approveWithClient(client(), title, options);
}

/** A handle to an existing durable wait. */
export function waiter(waiterRef: string): Waiter {
  return client().waiter(waiterRef);
}

/** Reattach to a wait a previous process was holding (§8.5). The only thing that had to survive is
 *  the `waiterRef`. */
export function resume(waiterRef: string): Promise<Waiter> {
  return client().resume(waiterRef);
}

/** What the configured deployment supports (§19). */
export function meta(): Promise<Meta> {
  return client().meta();
}

/** Spend one decision on exactly one effect, idempotently per `effectKey` (§10). */
export function redeem(
  authorizationId: string,
  effectKey: string,
  options: { effectDigest?: string } = {},
): Promise<RedeemResult> {
  return client().redeem(authorizationId, effectKey, options);
}
