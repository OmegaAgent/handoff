/**
 * A minimal in-memory Handoff server, for testing this SDK over a real socket.
 *
 * **This is a test double, not a conforming implementation.** It implements the handful of
 * operations these tests exercise and none of the guarantees that make a server conformant: no
 * tenancy, no authority evaluation, no receipt chain, no storage-level immutability, no delivery
 * ladder. The reference server is `core/`; the conformance suite is `conformance/`.
 *
 * It exists so the resumable-client tests run against real HTTP with a real long poll, which is the
 * only way to test that killing a client mid-wait loses nothing.
 */

import { createServer, type Server } from "node:http";

const CROCKFORD = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";

function ulid(prefix: string): string {
  const entropy = crypto.getRandomValues(new Uint8Array(10));
  let hex = "";
  for (const byte of entropy) hex += byte.toString(16).padStart(2, "0");
  let value = (BigInt(Date.now()) << 80n) | BigInt("0x" + hex);
  let out = "";
  for (let i = 0; i < 26; i += 1) {
    out = CROCKFORD[Number(value & 31n)] + out;
    value >>= 5n;
  }
  return `${prefix}_${out}`;
}

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

/** Everything the double remembers. The point of the exercise: the wait lives here, not in any
 *  client process. */
export class State {
  requests = new Map<string, any>();
  signals = new Map<string, any>();
  byWaiter = new Map<string, string[]>();
  sequence = new Map<string, number>();
  authorizations = new Map<string, any>();
  redemptions = new Map<string, Map<string, string>>();
  reattachCalls: string[] = [];
  ackCalls: Array<{ signalId: string; applied: boolean; reason?: string }> = [];

  raiseRequest(body: any) {
    const id = ulid("req");
    const request = {
      id,
      state: "pending",
      version: 1,
      org_id: "org_TEST",
      waiter_ref: body.waiter_ref,
      prompt: body.prompt,
      requires: body.requires,
      created_at: "2026-07-30T14:02:11Z",
      surface_url: `https://handoff.test/requests/${id}`,
      deliveries: [],
      receipt: null,
      authorization: null,
      waiter: { state: "armed", liveness: body.liveness ?? "durable" },
      metadata: body.metadata ?? null,
      resume_ref: body.resume_ref ?? null,
      resume_payload: body.resume_payload ?? null,
    };
    this.requests.set(id, request);
    if (!this.byWaiter.has(body.waiter_ref)) this.byWaiter.set(body.waiter_ref, []);
    return request;
  }

  /** Signals are a queue, not a flag (W2): a nudge never overwrites a later terminal signal. */
  enqueue(requestId: string, type: string, decision: any) {
    const request = this.requests.get(requestId);
    const waiterRef = request.waiter_ref;
    const next = (this.sequence.get(waiterRef) ?? 0) + 1;
    this.sequence.set(waiterRef, next);
    const signal = {
      id: ulid("sig"),
      request_id: requestId,
      waiter_ref: waiterRef,
      type,
      sequence: next,
      resume_token: ulid("rt"),
      decision,
      resume_ref: request.resume_ref,
      resume_payload: request.resume_payload,
      attempts: 1,
      created_at: "2026-07-30T14:07:44Z",
      acked_at: null,
    };
    this.signals.set(signal.id, signal);
    this.byWaiter.get(waiterRef)!.push(signal.id);
    return signal;
  }

  answer(requestId: string, body: any) {
    const request = this.requests.get(requestId);
    if (!request) throw new HttpError(404, "request_not_found", "no such request");
    if (request.state !== "pending") {
      throw new HttpError(409, "already_answered", `${requestId} is ${request.state}`);
    }
    request.state = "answered";
    request.answered_at = "2026-07-30T14:07:44Z";
    const receiptId = ulid("rcpt");
    const authorizationId = ulid("auth");
    this.authorizations.set(authorizationId, {
      id: authorizationId,
      receipt_id: receiptId,
      request_id: requestId,
      grants: body.values ?? {},
      single_use: true,
      expires_at: "2026-07-31T14:07:44Z",
    });
    this.enqueue(requestId, "answered", {
      outcome: "answered",
      values: body.values ?? {},
      source: "human",
      effective: null,
      receipt_id: receiptId,
      authorization_id: authorizationId,
      superseded_by: null,
    });
    return {
      request: { id: requestId, state: "answered", answered_at: "2026-07-30T14:07:44Z" },
      receipt: { id: receiptId, digest: "sha256:" + "0".repeat(64) },
      authorization: { id: authorizationId, single_use: true, expires_at: "2026-07-31T14:07:44Z" },
    };
  }

  unacked(waiterRef: string) {
    return (this.byWaiter.get(waiterRef) ?? [])
      .map((id) => this.signals.get(id))
      .filter((signal) => signal.acked_at === null);
  }

  ack(signalId: string, body: any) {
    const signal = this.signals.get(signalId);
    if (!signal) throw new HttpError(404, "signal_not_found", `${signalId} does not exist`);
    if (body.resume_token !== signal.resume_token) {
      throw new HttpError(403, "insufficient_scope", "resume_token does not match");
    }
    const first = signal.acked_at === null;
    this.ackCalls.push({ signalId, applied: Boolean(body.applied), reason: body.reason });
    if (first) signal.acked_at = "2026-07-30T14:07:50Z";
    return { acked_at: signal.acked_at, first_ack: first };
  }

  redeem(authorizationId: string, body: any) {
    if (!this.authorizations.has(authorizationId)) {
      throw new HttpError(404, "authorization_not_found", "no such authorization");
    }
    if (!this.redemptions.has(authorizationId)) this.redemptions.set(authorizationId, new Map());
    const spent = this.redemptions.get(authorizationId)!;
    const key = body.effect_key;
    if (spent.has(key)) return { redeemed_at: spent.get(key), first_redemption: false };
    if (spent.size > 0 && this.authorizations.get(authorizationId).single_use) {
      throw new HttpError(409, "authorization_spent", "single-use authorization already spent");
    }
    spent.set(key, "2026-07-30T14:07:46Z");
    return { redeemed_at: spent.get(key), first_redemption: true };
  }
}

class HttpError extends Error {
  readonly status: number;
  readonly code: string;

  // Written out rather than as parameter properties: Node's strip-only TypeScript support erases
  // types, it does not emit code, so `constructor(readonly x: T)` has nothing to erase down to.
  constructor(status: number, code: string, message: string) {
    super(message);
    this.status = status;
    this.code = code;
  }
}

/** Runs the double on a free port until `close()`. */
export class FakeServer {
  readonly state = new State();
  private server: Server;
  private port = 0;

  constructor() {
    this.server = createServer((req, res) => {
      void this.route(req, res);
    });
  }

  get baseUrl(): string {
    return `http://127.0.0.1:${this.port}`;
  }

  async start(): Promise<this> {
    await new Promise<void>((resolve) => this.server.listen(0, "127.0.0.1", resolve));
    this.port = (this.server.address() as any).port;
    return this;
  }

  async close(): Promise<void> {
    await new Promise<void>((resolve) => this.server.close(() => resolve()));
  }

  private async readBody(req: any): Promise<any> {
    const chunks: Buffer[] = [];
    for await (const chunk of req) chunks.push(chunk);
    const raw = Buffer.concat(chunks).toString();
    return raw ? JSON.parse(raw) : {};
  }

  private send(res: any, status: number, payload?: unknown): void {
    if (payload === undefined) {
      res.writeHead(status);
      res.end();
      return;
    }
    const body = JSON.stringify(payload);
    res.writeHead(status, { "Content-Type": "application/json", "Content-Length": Buffer.byteLength(body) });
    res.end(body);
  }

  private async route(req: any, res: any): Promise<void> {
    const url = new URL(req.url, "http://127.0.0.1");
    const parts = url.pathname.split("/").filter(Boolean).map(decodeURIComponent);
    try {
      if (req.method === "GET" && parts[0] === "v1" && parts[1] === "meta") {
        return this.send(res, 200, {
          protocol_version: "0.1",
          conformance_level: 1,
          extensions: [],
          field_types: ["choice", "text", "number", "boolean", "secret", "attestation"],
          capability_types: ["interactive_surface"],
          max_wait_seconds: 30,
        });
      }
      if (req.method === "POST" && parts.length === 2 && parts[1] === "requests") {
        return this.send(res, 201, this.state.raiseRequest(await this.readBody(req)));
      }
      if (req.method === "POST" && parts[3] === "answer") {
        return this.send(res, 200, this.state.answer(parts[2], await this.readBody(req)));
      }
      if (req.method === "GET" && parts[1] === "waiters" && parts[3] === "signals") {
        const wait = Number(url.searchParams.get("wait") ?? 0);
        const deadline = Date.now() + wait * 1000;
        for (;;) {
          const pending = this.state.unacked(parts[2]);
          if (pending.length > 0) return this.send(res, 200, { data: pending, has_more: false });
          if (Date.now() >= deadline) return this.send(res, 204);
          await sleep(50);
        }
      }
      if (req.method === "POST" && parts[1] === "waiters" && parts[3] === "reattach") {
        this.state.reattachCalls.push(parts[2]);
        const pending = this.state.unacked(parts[2]);
        return this.send(res, 200, {
          waiter_ref: parts[2],
          state: pending.length > 0 ? "signalled" : "armed",
          open_requests: [...this.state.requests.values()]
            .filter((r) => r.waiter_ref === parts[2] && r.state === "pending")
            .map((r) => r.id),
          signals: pending,
        });
      }
      if (req.method === "POST" && parts[1] === "signals" && parts[3] === "ack") {
        return this.send(res, 200, this.state.ack(parts[2], await this.readBody(req)));
      }
      if (req.method === "POST" && parts[1] === "authorizations" && parts[3] === "redeem") {
        return this.send(res, 200, this.state.redeem(parts[2], await this.readBody(req)));
      }
      if (req.method === "GET" && parts[1] === "requests" && parts.length === 3) {
        const request = this.state.requests.get(parts[2]);
        if (!request) throw new HttpError(404, "request_not_found", parts[2]);
        return this.send(res, 200, request);
      }
      return this.send(res, 404, { error: { code: "request_not_found", message: req.url } });
    } catch (error) {
      if (error instanceof HttpError) {
        return this.send(res, error.status, { error: { code: error.code, message: error.message } });
      }
      return this.send(res, 500, { error: { code: "invalid_request", message: String(error) } });
    }
  }
}

export async function withServer<T>(fn: (server: FakeServer) => Promise<T>): Promise<T> {
  const server = await new FakeServer().start();
  try {
    return await fn(server);
  } finally {
    await server.close();
  }
}
