/**
 * The resumable client: receive, apply, ack.
 *
 * The failure this exists to prevent is the obvious one. If the wait lives inside the agent
 * process, the wait dies with the process, and a person's answer lands somewhere nobody is
 * listening. In this protocol the wait is a durable server-side row (§8) and the client is a
 * disposable reader of it.
 *
 * So the SDK offers two shapes over the same durable wait: a blocking call that long-polls in
 * server-capped windows until a deadline, and a resumable one where the process may die at any
 * point and a later process reattaches by `waiterRef`, receives the still-unacked signal, and acks
 * it idempotently.
 *
 * The ack is the hinge. Delivery is at-least-once and the ack is idempotent, and those two
 * together give effectively-once *application* — but only if the ack happens after the outcome has
 * actually been applied. `Waiter.receive` is built so the ordering is not something the caller has
 * to remember: the ack is sent when the callback resolves, and a rejection leaves the signal
 * unacked and redeliverable.
 *
 * What this does not do, and no implementation of this protocol can, is resume your execution. The
 * defensible claim is narrower and worth more: one human answer, delivered to your agent exactly
 * once, as typed data, authorizing exactly one effect.
 */

import { HandoffTimeout, SignalNotApplied, TransportError } from "./errors.ts";
import type { AckResult, Decision, ReattachResult, Request, Signal } from "./models.ts";
import type { Client } from "./client.ts";

const sleep = (ms: number) => new Promise<void>((resolve) => setTimeout(resolve, ms));

/**
 * The typed result of one intervention.
 *
 * Data the runtime reads, never an instruction it must obey. `values` never carries a secret: a
 * `secret` field arrives as `{"provided": true}` and the value itself went to the sink the runtime
 * owns (§12, I7).
 */
export class Outcome {
  readonly signal: Signal;
  private readonly client: Client | undefined;
  private readonly truthy: boolean | undefined;

  constructor(signal: Signal, client?: Client, truthy?: boolean) {
    this.signal = signal;
    this.client = client;
    this.truthy = truthy;
  }

  get decision(): Decision | null {
    return this.signal.decision;
  }

  /** `answered`, `expired`, `cancelled`, `superseded` — or `attempt_lapsed` for the non-terminal
   *  nudge, which decides nothing. */
  get outcome(): string {
    return this.signal.decision?.outcome ?? this.signal.type;
  }

  get values(): Record<string, any> {
    return this.signal.decision?.values ?? {};
  }

  get source(): string | null {
    return this.signal.decision?.source ?? null;
  }

  /** True only where a person decided. A policy expiry and a runtime inference are both legitimate
   *  outcomes and neither is a person (§9.6, I16). */
  get decidedByHuman(): boolean {
    return this.source === "human";
  }

  get receiptId(): string | null {
    return this.signal.decision?.receiptId ?? null;
  }

  get authorizationId(): string | null {
    return this.signal.decision?.authorizationId ?? null;
  }

  get approved(): boolean {
    return this.truthy ?? this.outcome === "answered";
  }

  value<T = any>(name: string, fallback?: T): T {
    const values = this.values;
    return (name in values ? values[name] : fallback) as T;
  }

  /**
   * Spend this decision on exactly one effect, idempotently per `effectKey` (§10).
   *
   * Call it immediately before performing the effect and act only when `firstRedemption` is true.
   * That is what stops a replayed turn from refunding twice.
   */
  redeem(effectKey: string, options: { effectDigest?: string } = {}) {
    if (!this.client) throw new Error("this Outcome was not produced by a Client and cannot redeem");
    const authorizationId = this.authorizationId;
    if (!authorizationId) {
      throw new Error(`outcome '${this.outcome}' minted no authorization; there is nothing to spend`);
    }
    return this.client.redeem(authorizationId, effectKey, options);
  }
}

/**
 * One signal handed to the caller, not yet acked.
 *
 * Let the callback resolve and the signal is acked as applied. Call `unable()` (or throw
 * `SignalNotApplied`) and it is acked as *not* applied, with the reason recorded. Let it reject and
 * nothing is acked at all, so the server keeps it and the next process to reattach still finds it.
 */
export class Received {
  readonly signal: Signal;
  readonly outcome: Outcome;
  applied = true;
  reason: string | undefined;

  constructor(signal: Signal, client?: Client) {
    this.signal = signal;
    this.outcome = new Outcome(signal, client);
  }

  /** Record that the decision arrived and could not be acted on. Not an error: the server accepts
   *  it, stops redelivery, and keeps the fact (§8.3). */
  unable(reason: string): void {
    this.applied = false;
    this.reason = reason;
  }

  get decision(): Decision | null {
    return this.signal.decision;
  }

  get values(): Record<string, any> {
    return this.outcome.values;
  }
}

export interface NextOptions {
  /** Milliseconds. Omit to wait indefinitely. */
  timeoutMs?: number;
  accept?: (signal: Signal) => boolean;
  /** Seconds. Defaults to the server's advertised long-poll cap. */
  pollWaitSeconds?: number;
}

/**
 * A handle to one durable server-side wait.
 *
 * Constructing this registers nothing and costs nothing. The waiter itself was created server-side
 * by the raise (§8.2 W1) and outlives every process that reads it.
 */
export class Waiter {
  readonly client: Client;
  readonly waiterRef: string;
  private buffer: Signal[] = [];
  private highest = 0;

  constructor(client: Client, waiterRef: string, buffered: Signal[] = []) {
    this.client = client;
    this.waiterRef = waiterRef;
    this.buffer = [...buffered];
  }

  /** Poll for unacked signals. Reading does not consume them (§8.3). */
  async signals(waitSeconds = 0): Promise<Signal[]> {
    return this.client.pollSignals(this.waiterRef, waitSeconds);
  }

  /** Re-arm the lease and collect every unacked signal (§8.5, W7). Signals are buffered locally so
   *  the next `receive` hands them over without another round trip. */
  async reattach(): Promise<ReattachResult> {
    const result = await this.client.reattach(this.waiterRef);
    const known = new Set(this.buffer.map((s) => s.id));
    for (const signal of result.signals) if (!known.has(signal.id)) this.buffer.push(signal);
    return result;
  }

  /** Consume a signal. Safe to call twice: the second returns `firstAck: false`. */
  async ack(signal: Signal, options: { applied?: boolean; reason?: string } = {}): Promise<AckResult> {
    if (typeof signal === "string") {
      throw new TypeError(
        "ack() needs the Signal itself: the resume_token that authorizes the ack travels on the " +
          "signal and is deliberately not something you can pass by id",
      );
    }
    return this.client.ack(signal.id, signal.resumeToken, options);
  }

  /** The highest sequence this handle has handed out.
   *
   * Sequence is monotonic per `waiterRef`, so a gap means a signal is in flight or was reordered.
   * A gap is not by itself an error — delivery is at-least-once and retries reorder — but one that
   * never closes is worth raising operationally (signing.md §1.3).
   */
  get highestSequence(): number {
    return this.highest;
  }

  /**
   * Block until a signal is available and return it **unacked**.
   *
   * Long-polls in windows the server caps (`meta.max_wait_seconds`), looping until the timeout.
   * Hanging up between windows does not affect the durable wait, so a dropped connection costs one
   * window and nothing else.
   *
   * Signals that `accept` rejects are left in the queue untouched — never acked, never dropped —
   * because a waiter may hold signals for several requests and retiring one the caller did not ask
   * about would lose it.
   */
  async next(options: NextOptions = {}): Promise<Signal> {
    const deadline = options.timeoutMs === undefined ? undefined : Date.now() + options.timeoutMs;
    const window = options.pollWaitSeconds ?? (await this.client.maxWaitSeconds());
    let failures = 0;

    for (;;) {
      const index = this.buffer.findIndex((s) => !options.accept || options.accept(s));
      if (index >= 0) {
        const [signal] = this.buffer.splice(index, 1);
        this.highest = Math.max(this.highest, signal.sequence ?? 0);
        return signal;
      }

      const remainingMs = deadline === undefined ? undefined : deadline - Date.now();
      if (remainingMs !== undefined && remainingMs <= 0) {
        throw new HandoffTimeout(
          `no matching signal for '${this.waiterRef}' within the local deadline; the durable wait ` +
            "is unaffected and reattaching later will still find it",
          { waiterRef: this.waiterRef },
        );
      }
      const thisWindow =
        remainingMs === undefined ? window : Math.max(1, Math.min(window, Math.floor(remainingMs / 1000)));

      let found: Signal[];
      try {
        found = await this.signals(thisWindow);
        failures = 0;
      } catch (error) {
        if (!(error instanceof TransportError)) throw error;
        // The wait is on the server. Losing the connection to it is a retry, not a loss.
        failures += 1;
        if (failures >= 5) throw error;
        await sleep(Math.min(2 ** failures, 10) * 1000);
        continue;
      }

      const known = new Set(this.buffer.map((s) => s.id));
      for (const signal of found) if (!known.has(signal.id)) this.buffer.push(signal);
    }
  }

  /**
   * Receive one signal, apply it in the callback, and ack on the way out.
   *
   * ```ts
   * await waiter.receive(async (received) => {
   *   await apply(received.values);   // your work
   * });                               // the ack is sent here
   * ```
   *
   * The ack is sent only after the callback resolves. If it rejects, nothing is acked and the
   * signal stays queued for the next process to reattach and find. That ordering is the point:
   * acking first and applying second would turn at-least-once delivery into at-most-once
   * application, which is the bug this protocol exists to make impossible.
   *
   * Throwing `SignalNotApplied` inside the callback acks with `applied: false` and the reason, and
   * does not propagate — it is a way to record non-application from deep in a call stack, and the
   * recording is the handling.
   */
  async receive<T>(
    apply: (received: Received) => T | Promise<T>,
    options: NextOptions = {},
  ): Promise<T | undefined> {
    const signal = await this.next(options);
    const received = new Received(signal, this.client);
    let result: T;
    try {
      result = await apply(received);
    } catch (error) {
      if (error instanceof SignalNotApplied) {
        await this.ack(signal, { applied: false, reason: error.reason });
        return undefined;
      }
      throw error; // deliberately no ack: unapplied means undelivered
    }
    await this.ack(signal, { applied: received.applied, reason: received.reason });
    return result;
  }

  /**
   * Block until a **terminal** signal arrives and return it unacked.
   *
   * `attempt_lapsed` is a nudge, not an outcome: the request is still pending and still answerable,
   * and the person has simply gone quiet for a while (§6.3). It is reported to `onAttemptLapsed`
   * and then acked, because there is no outcome to apply and leaving it unacked only makes the
   * server redeliver a notification you have already seen. Pass `ackNudges: false` to leave it.
   */
  async waitForOutcome(
    options: {
      requestId?: string;
      timeoutMs?: number;
      onAttemptLapsed?: (signal: Signal) => void;
      ackNudges?: boolean;
    } = {},
  ): Promise<Signal> {
    const deadline = options.timeoutMs === undefined ? undefined : Date.now() + options.timeoutMs;
    for (;;) {
      const remainingMs = deadline === undefined ? undefined : Math.max(0, deadline - Date.now());
      const signal = await this.next({
        timeoutMs: remainingMs,
        accept: (s) => options.requestId === undefined || s.requestId === options.requestId,
      });
      if (signal.isTerminal) return signal;
      options.onAttemptLapsed?.(signal);
      if (options.ackNudges !== false) await this.ack(signal, { applied: true });
    }
  }
}

/**
 * A raised request and the durable wait registered with it.
 *
 * Hold on to `waiterRef`, not to this object. It is the `waiterRef` that a later process needs, and
 * it is the only thing that has to survive.
 */
export class PendingRequest {
  readonly request: Request;
  readonly waiter: Waiter;
  /** The 201/200 distinction, and it is contract: how a caller tells "I asked" from "I already
   *  asked" (§3.3). */
  readonly wasCreated: boolean;
  private readonly client: Client;

  constructor(client: Client, request: Request, wasCreated: boolean) {
    this.client = client;
    this.request = request;
    this.wasCreated = wasCreated;
    this.waiter = new Waiter(client, request.waiterRef);
  }

  get id(): string {
    return this.request.id;
  }

  get waiterRef(): string {
    return this.request.waiterRef;
  }

  /** Where a person goes to answer. A locator, not a capability: opening it prompts for
   *  authentication and holding it authorizes nothing (§4.6). */
  get surfaceUrl(): string | undefined {
    return this.request.surfaceUrl;
  }

  /** Block for this request's terminal signal. Returns it **unacked**. */
  async wait(
    options: { timeoutMs?: number; onAttemptLapsed?: (signal: Signal) => void } = {},
  ): Promise<Signal> {
    return this.waiter.waitForOutcome({ ...options, requestId: this.id });
  }

  /** Wait for this request's outcome, apply it in the callback, ack on the way out. */
  async receive<T>(
    apply: (received: Received) => T | Promise<T>,
    options: { timeoutMs?: number; onAttemptLapsed?: (signal: Signal) => void } = {},
  ): Promise<T | undefined> {
    const signal = await this.wait(options);
    const received = new Received(signal, this.client);
    let result: T;
    try {
      result = await apply(received);
    } catch (error) {
      if (error instanceof SignalNotApplied) {
        await this.waiter.ack(signal, { applied: false, reason: error.reason });
        return undefined;
      }
      throw error;
    }
    await this.waiter.ack(signal, { applied: received.applied, reason: received.reason });
    return result;
  }

  async cancel(reason: string): Promise<Request> {
    return this.client.cancel(this.id, reason);
  }
}
