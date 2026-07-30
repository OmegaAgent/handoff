/**
 * The HTTP client. No runtime dependencies — `fetch` and `node:crypto` only.
 *
 * What this client does *not* own is the wait. The wait is a durable row on the server, keyed by
 * `waiterRef` (§8). This object is a handle to it and is cheap to throw away: a later process that
 * knows the `waiterRef` reattaches and finds the same unacked signals. That is the whole difference
 * between a protocol and a loop.
 */

import { compact, toHex, type JsonObject } from "./document.ts";
import { fromErrorBody, HandoffError, TransportError } from "./errors.ts";
import {
  AckResult,
  AnswerResult,
  Authorization,
  Meta,
  ReattachResult,
  RedeemResult,
  Request,
  Signal,
  type AuthStrength,
  type Disposition,
} from "./models.ts";
import { PendingRequest, Waiter } from "./waiter.ts";

export const DEFAULT_BASE_URL = "https://handoff.omegas.dev";

const CROCKFORD = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/**
 * A fresh time-sortable key.
 *
 * Reused verbatim across this client's own transport retries, so a retry that reaches the server
 * after a response was lost returns the stored representation instead of raising a second ask
 * (§3.3, §3.5).
 */
export function newIdempotencyKey(): string {
  const entropy = crypto.getRandomValues(new Uint8Array(10));
  let value = (BigInt(Date.now()) << 80n) | BigInt("0x" + toHex(entropy));
  let out = "";
  for (let i = 0; i < 26; i += 1) {
    out = CROCKFORD[Number(value & 31n)] + out;
    value >>= 5n;
  }
  return out;
}

export interface ClientOptions {
  baseUrl?: string;
  apiKey?: string;
  /** Milliseconds. */
  timeoutMs?: number;
  userAgent?: string;
  maxTransportRetries?: number;
  fetch?: typeof globalThis.fetch;
}

export interface RaiseOptions {
  waiterRef: string;
  prompt: JsonObject;
  requires: JsonObject;
  liveness?: "durable" | "leased";
  urgency?: "low" | "normal" | "high" | "critical";
  ttl?: string;
  ttlPolicy?: JsonObject;
  attemptTtl?: string;
  onWaiterTerminal?: "keep" | "cancel";
  routing?: JsonObject;
  mode?: "advisory" | "gated";
  presentationBinding?: "advisory" | "strict";
  dedupeKey?: string;
  /** Level 2 (`continuation`). Stored verbatim, returned byte-identical, interpreted never (§14). */
  resumeRef?: string;
  resumePayload?: string;
  callback?: JsonObject;
  metadata?: JsonObject;
  testMode?: boolean;
  idempotencyKey?: string;
}

/**
 * A Handoff API client.
 *
 * `apiKey` authenticates an org-scoped service account. It can raise, read, poll, ack, and redeem —
 * and it can never answer, by principal type and under no configuration (§4.2, I15).
 */
export class Client {
  readonly baseUrl: string;
  readonly timeoutMs: number;
  readonly userAgent: string;
  readonly maxTransportRetries: number;
  /** Held privately so a stray log line, inspect, or stack trace cannot print it (I18). */
  readonly #apiKey: string | undefined;
  readonly #fetch: typeof globalThis.fetch;
  #maxWait: number | undefined;

  constructor(options: ClientOptions = {}) {
    const env = (globalThis as any).process?.env ?? {};
    this.baseUrl = (options.baseUrl ?? env.HANDOFF_URL ?? DEFAULT_BASE_URL).replace(/\/+$/, "");
    this.#apiKey = options.apiKey ?? env.HANDOFF_API_KEY;
    this.timeoutMs = options.timeoutMs ?? 30_000;
    this.userAgent = options.userAgent ?? "handoff-ts/0.2.0";
    this.maxTransportRetries = options.maxTransportRetries ?? 3;
    this.#fetch = options.fetch ?? globalThis.fetch.bind(globalThis);
  }

  toString(): string {
    return `Client(baseUrl=${this.baseUrl}, apiKey=<redacted>)`;
  }

  [Symbol.for("nodejs.util.inspect.custom")](): string {
    return this.toString();
  }

  /** One request. Returns `[status, parsedBody]`; `204` parses as `null`.
   *
   *  Secrets never reach the URL: query values are for wait windows and cursors only (I18). */
  async call(
    method: string,
    path: string,
    options: {
      body?: JsonObject;
      idempotencyKey?: string;
      timeoutMs?: number;
      query?: Record<string, string | number | undefined>;
    } = {},
  ): Promise<[number, any]> {
    let url = this.baseUrl + path;
    if (options.query) {
      const params = new URLSearchParams();
      for (const [key, value] of Object.entries(options.query)) {
        if (value !== undefined) params.set(key, String(value));
      }
      const encoded = params.toString();
      if (encoded) url += "?" + encoded;
    }

    const headers: Record<string, string> = {
      Accept: "application/json",
      "User-Agent": this.userAgent,
    };
    if (options.body !== undefined) headers["Content-Type"] = "application/json";
    if (this.#apiKey) headers.Authorization = `Bearer ${this.#apiKey}`;
    if (options.idempotencyKey) headers["Idempotency-Key"] = options.idempotencyKey;

    const attempts = Math.max(1, this.maxTransportRetries);
    let last: unknown;

    for (let attempt = 0; attempt < attempts; attempt += 1) {
      const controller = new AbortController();
      const timer = setTimeout(() => controller.abort(), options.timeoutMs ?? this.timeoutMs);
      try {
        const response = await this.#fetch(url, {
          method,
          headers,
          body: options.body === undefined ? undefined : JSON.stringify(options.body),
          signal: controller.signal,
        });
        const text = await response.text();
        if (!response.ok) throw await this.#errorFrom(response, text);
        if (response.status === 204 || !text) return [response.status, null];
        return [response.status, JSON.parse(text)];
      } catch (error) {
        if (error instanceof HandoffError) throw error;
        // A dropped long poll is expected, not exceptional: the durable wait is unaffected and
        // re-polling picks up exactly where this left off.
        last = error;
        if (attempt + 1 < attempts) {
          await new Promise((resolve) =>
            setTimeout(resolve, Math.min(2 ** attempt, 8) * 1000 * (0.5 + Math.random() / 2)),
          );
          continue;
        }
        throw new TransportError(`${method} ${path} could not be completed: ${(error as Error).message}`);
      } finally {
        clearTimeout(timer);
      }
    }
    throw new TransportError(`${method} ${path} failed: ${String(last)}`);
  }

  async #errorFrom(response: Response, text: string): Promise<HandoffError> {
    let body: any = {};
    try {
      body = text ? JSON.parse(text) : {};
    } catch {
      body = {};
    }
    const rawRetry = response.headers.get("Retry-After");
    const retryAfter = rawRetry && !Number.isNaN(Number(rawRetry)) ? Number(rawRetry) : undefined;
    if (!body || typeof body !== "object" || !body.error) {
      return new TransportError(`HTTP ${response.status} without the protocol error envelope`, {
        status: response.status,
        retryAfter,
      });
    }
    return fromErrorBody(body, { status: response.status, retryAfter });
  }

  // -- discovery -------------------------------------------------------------------------------

  /** What this deployment supports (§19). Unauthenticated. */
  async meta(): Promise<Meta> {
    const [, body] = await this.call("GET", "/v1/meta");
    const meta = new Meta(body);
    this.#maxWait = meta.maxWaitSeconds;
    return meta;
  }

  /** The server's long-poll cap, discovered once and cached. Falls back to the protocol's
   *  documented 30s if discovery is unavailable — a larger value is clamped, not rejected. */
  async maxWaitSeconds(): Promise<number> {
    if (this.#maxWait === undefined) {
      try {
        await this.meta();
      } catch {
        this.#maxWait = 30;
      }
    }
    return this.#maxWait ?? 30;
  }

  // -- requests --------------------------------------------------------------------------------

  /**
   * Raise a request and return a handle to it and its durable wait.
   *
   * The raise does not block on delivery. Deliveries come back `queued`, and a channel outage never
   * takes the caller's agent down with it (§7.3).
   */
  async raiseRequest(options: RaiseOptions): Promise<PendingRequest> {
    const body = compact({
      waiter_ref: options.waiterRef,
      liveness: options.liveness,
      urgency: options.urgency,
      prompt: options.prompt,
      requires: options.requires,
      ttl: options.ttl,
      ttl_policy: options.ttlPolicy,
      attempt_ttl: options.attemptTtl,
      on_waiter_terminal: options.onWaiterTerminal,
      routing: options.routing,
      mode: options.mode,
      presentation_binding: options.presentationBinding,
      dedupe_key: options.dedupeKey,
      resume_ref: options.resumeRef,
      resume_payload: options.resumePayload,
      callback: options.callback,
      metadata: options.metadata,
      test_mode: options.testMode,
    });
    const [status, response] = await this.call("POST", "/v1/requests", {
      body,
      idempotencyKey: options.idempotencyKey ?? newIdempotencyKey(),
    });
    return new PendingRequest(this, new Request(response), status === 201);
  }

  async getRequest(requestId: string): Promise<Request> {
    const [, body] = await this.call("GET", `/v1/requests/${encodeURIComponent(requestId)}`);
    return new Request(body);
  }

  /** Withdraw the ask. A landed answer wins the race and this returns `409 already_answered` — a
   *  machine changing its mind must not discard a person's work (R11). */
  async cancel(requestId: string, reason: string): Promise<Request> {
    const [, body] = await this.call("POST", `/v1/requests/${encodeURIComponent(requestId)}/cancel`, {
      body: { reason },
      idempotencyKey: newIdempotencyKey(),
    });
    return new Request(body);
  }

  /**
   * Settle a request. **Human principals only.**
   *
   * A machine key gets `403 requester_may_not_answer`, by principal type rather than by role or
   * setting (§4.2). This method exists for clients holding a person's own session — a surface, or a
   * test harness standing in for one — and calling it with a service-account key is expected to
   * fail.
   */
  async answer(
    requestId: string,
    values: JsonObject,
    options: {
      viaDeliveryId?: string;
      partial?: boolean;
      note?: string;
      disposition?: Disposition;
      renderedDigest?: string;
      idempotencyKey?: string;
    } = {},
  ): Promise<AnswerResult> {
    const body = compact({
      values,
      via_delivery_id: options.viaDeliveryId,
      partial: options.partial,
      note: options.note,
      disposition: options.disposition,
      rendered_digest: options.renderedDigest,
    });
    const [, response] = await this.call("POST", `/v1/requests/${encodeURIComponent(requestId)}/answer`, {
      body,
      idempotencyKey: options.idempotencyKey ?? newIdempotencyKey(),
    });
    return new AnswerResult(response);
  }

  // -- waiters ---------------------------------------------------------------------------------

  /**
   * Every unacked signal for this waiter, oldest first. **Reading does not consume.**
   *
   * Consumption is the ack. A client that reads a signal and then dies has not received it, and the
   * server keeps it until an explicit ack arrives (§8.3).
   */
  async pollSignals(waiterRef: string, waitSeconds = 0): Promise<Signal[]> {
    const [status, body] = await this.call("GET", `/v1/waiters/${encodeURIComponent(waiterRef)}/signals`, {
      query: waitSeconds ? { wait: waitSeconds } : undefined,
      timeoutMs: this.timeoutMs + waitSeconds * 1000,
    });
    if (status === 204 || !body) return [];
    return (body.data ?? []).map((s: JsonObject) => new Signal(s));
  }

  /**
   * Re-arm this waiter and collect everything it is still holding (§8.5, W7).
   *
   * This is the operation that makes a client's own process death survivable. The wait was never in
   * that process, so nothing was lost while it was gone.
   */
  async reattach(waiterRef: string): Promise<ReattachResult> {
    const [, body] = await this.call("POST", `/v1/waiters/${encodeURIComponent(waiterRef)}/reattach`, {
      body: {},
      idempotencyKey: newIdempotencyKey(),
    });
    return new ReattachResult(body);
  }

  /** A handle to an existing durable wait. Registers nothing; costs nothing. */
  waiter(waiterRef: string): Waiter {
    return new Waiter(this, waiterRef);
  }

  /**
   * Reattach to a wait a previous process was holding, and return a handle carrying everything it
   * was still owed (§8.5).
   *
   * This is the whole restart recipe. The only thing that had to survive the crash is the
   * `waiterRef` string.
   */
  async resume(waiterRef: string): Promise<Waiter> {
    const waiter = new Waiter(this, waiterRef);
    await waiter.reattach();
    return waiter;
  }

  /**
   * Consume a signal. Idempotent: `firstAck` is true then false, and redelivery stops once.
   *
   * `applied: false` with a reason is not an error. It records that the decision arrived and could
   * not be acted on, which is a fact worth holding rather than swallowing (§8.3).
   */
  async ack(
    signalId: string,
    resumeToken: string,
    options: { applied?: boolean; reason?: string } = {},
  ): Promise<AckResult> {
    const body = compact({
      resume_token: resumeToken,
      applied: options.applied ?? true,
      reason: options.reason,
    });
    const [, response] = await this.call("POST", `/v1/signals/${encodeURIComponent(signalId)}/ack`, { body });
    return new AckResult(response);
  }

  // -- authorizations --------------------------------------------------------------------------

  async getAuthorization(authorizationId: string): Promise<Authorization> {
    const [, body] = await this.call("GET", `/v1/authorizations/${encodeURIComponent(authorizationId)}`);
    return new Authorization(body);
  }

  /**
   * Spend one decision on exactly one effect.
   *
   * Idempotent per `effectKey`: a replay returns `firstRedemption: false`, so a retried agent turn
   * cannot send the customer a second refund. The key must be **stable per effect** — one that
   * varies per attempt defeats the entire mechanism (§10, C-13).
   */
  async redeem(
    authorizationId: string,
    effectKey: string,
    options: { effectDigest?: string } = {},
  ): Promise<RedeemResult> {
    const body = compact({ effect_key: effectKey, effect_digest: options.effectDigest });
    const [, response] = await this.call(
      "POST",
      `/v1/authorizations/${encodeURIComponent(authorizationId)}/redeem`,
      { body },
    );
    return new RedeemResult(response);
  }

  // -- shorthands ------------------------------------------------------------------------------

  /** Ask a person a question and block until they answer. See `ask` in `ergonomics.ts`. */
  async ask(question: string, options: Record<string, any> = {}): Promise<string | undefined> {
    const { ask } = await import("./ergonomics.ts");
    return ask(this, question, options);
  }

  /** Ask a person to approve or reject, and block. See `approve` in `ergonomics.ts`. */
  async approve(title: string, options: Record<string, any> = {}) {
    const { approve } = await import("./ergonomics.ts");
    return approve(this, title, options);
  }
}

export type { AuthStrength };
