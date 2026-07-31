/**
 * The resumable client: the wait survives the process that started it.
 *
 * The prior art's fatal flaw was that the agent process owned the wait loop, so if the agent died
 * the wait died with it. These tests kill the waiting process — a real `SIGKILL` to a real
 * subprocess in the middle of a real long poll — and then prove that a later process reattaches by
 * `waiterRef` and finds the answer still there, still unacked.
 *
 * What is asserted here is the protocol's actual guarantee (§1.3): the answer reaches the runtime
 * as typed data, delivery is at-least-once with an idempotent ack, and one answer authorizes
 * exactly one effect. Not that execution resumed — nothing here claims that, because nothing can.
 */

import { test } from "node:test";
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

import { FakeServer, withServer } from "./fake-server.ts";
import {
  AuthorizationSpent,
  Client,
  HandoffTimeout,
  SignalNotApplied,
  type PendingRequest,
} from "../src/index.ts";

const HERE = dirname(fileURLToPath(import.meta.url));
const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

function raiseOne(client: Client, waiterRef = "run:0198f2a1"): Promise<PendingRequest> {
  return client.raiseRequest({
    waiterRef,
    prompt: { title: "Refund $2,400 to Acme Corp?" },
    requires: {
      v: 1,
      answer: { fields: [{ name: "decision", label: "Decision", type: "choice" }] },
      capabilities: [],
      authority: { min_role: "editor", auth_strength: "session" },
    },
  });
}

const agent = (server: FakeServer) => new Client({ baseUrl: server.baseUrl, apiKey: "test-key" });
const person = (server: FakeServer) => new Client({ baseUrl: server.baseUrl, apiKey: "a-persons-session" });

test("client death mid-wait loses nothing", async () => {
  // Kill -9 a process that is blocked in a long poll, then reattach from a fresh one. The
  // subprocess is a genuine separate interpreter, so nothing in this process's memory can be what
  // makes this work. The only thing that crosses the boundary is the waiterRef string.
  await withServer(async (server) => {
    const client = agent(server);
    const pending = await raiseOne(client);

    const child = spawn(
      process.execPath,
      [join(HERE, "wait-and-die.ts"), server.baseUrl, pending.waiterRef],
      { stdio: ["ignore", "pipe", "inherit"] },
    );

    await new Promise<void>((resolve, reject) => {
      let seen = "";
      child.stdout.on("data", (chunk) => {
        seen += String(chunk);
        if (seen.includes("polling")) resolve();
      });
      child.on("exit", (code) => reject(new Error(`child exited early with ${code}`)));
      setTimeout(() => reject(new Error("child never started polling")), 15_000);
    });
    await sleep(300);

    const exited = new Promise<number | null>((resolve) => child.on("exit", (_c, signal) => resolve(signal as any)));
    child.kill("SIGKILL");
    await exited;
    assert.equal(child.killed, true);

    // The person answers while nobody at all is listening.
    await person(server).answer(pending.id, {
      decision: "approve",
      note: "Confirmed with Acme on the phone.",
    });

    // A later process, with nothing but the waiterRef.
    const resumed = await agent(server).resume(pending.waiterRef);
    const signal = await resumed.next({ timeoutMs: 5_000 });

    assert.equal(signal.type, "answered");
    assert.equal(signal.ackedAt, null, "reading a signal must not consume it (§8.3)");
    assert.equal(signal.decision?.values.decision, "approve");
    assert.ok(signal.decision?.decidedByHuman);
    assert.deepEqual(server.state.reattachCalls, [pending.waiterRef]);
  });
});

test("an answer that lands before anyone reattaches is still waiting", async () => {
  await withServer(async (server) => {
    const pending = await raiseOne(agent(server), "run:no-listener");
    await person(server).answer(pending.id, { decision: "approve" });

    const result = await agent(server).reattach(pending.waiterRef);
    assert.equal(result.state, "signalled");
    assert.deepEqual(result.signals.map((s) => s.type), ["answered"]);
    assert.equal(result.signals[0].ackedAt, null);
  });
});

test("a double ack is idempotent", async () => {
  // C-12: both calls return 200, firstAck is true then false, redelivery stops once.
  await withServer(async (server) => {
    const client = agent(server);
    const pending = await raiseOne(client, "run:double-ack");
    await person(server).answer(pending.id, { decision: "approve" });

    const waiter = client.waiter(pending.waiterRef);
    const signal = await waiter.next({ timeoutMs: 5_000 });

    const first = await waiter.ack(signal);
    const second = await waiter.ack(signal);
    assert.equal(first.firstAck, true);
    assert.equal(second.firstAck, false);
    assert.equal(first.ackedAt, second.ackedAt);
    assert.deepEqual(await waiter.signals(), [], "an acked signal is consumed and stops being returned");
  });
});

test("receive acks only after the callback resolves", async () => {
  // The ordering that turns at-least-once delivery into effectively-once application.
  await withServer(async (server) => {
    const client = agent(server);
    const pending = await raiseOne(client, "run:receive-ok");
    await person(server).answer(pending.id, { decision: "approve" });

    const applied = await client.waiter(pending.waiterRef).receive(
      async (received) => {
        assert.deepEqual(server.state.ackCalls, [], "must not ack before the caller has applied anything");
        return received.values.decision;
      },
      { timeoutMs: 5_000 },
    );

    assert.equal(applied, "approve");
    assert.equal(server.state.ackCalls.length, 1);
    assert.equal(server.state.ackCalls[0].applied, true);
  });
});

test("a rejection in the callback leaves the signal unacked", async () => {
  // If applying the outcome failed, the outcome was not received. The signal stays queued and the
  // next process to reattach still finds it.
  await withServer(async (server) => {
    const client = agent(server);
    const pending = await raiseOne(client, "run:receive-boom");
    await person(server).answer(pending.id, { decision: "approve" });

    await assert.rejects(
      () =>
        client.waiter(pending.waiterRef).receive(
          async () => {
            throw new Error("downstream exploded");
          },
          { timeoutMs: 5_000 },
        ),
      /downstream exploded/,
    );

    assert.deepEqual(server.state.ackCalls, []);
    const resumed = await client.resume(pending.waiterRef);
    const survivor = await resumed.next({ timeoutMs: 5_000 });
    assert.equal(survivor.type, "answered");
    assert.equal(survivor.ackedAt, null);
  });
});

test("unable() records non-application without being an error", async () => {
  // §8.3: applied:false with a reason is a fact the record should hold, not an error to swallow.
  await withServer(async (server) => {
    const client = agent(server);
    const pending = await raiseOne(client, "run:unable");
    await person(server).answer(pending.id, { decision: "approve" });

    const waiter = client.waiter(pending.waiterRef);
    await waiter.receive(async (received) => received.unable("the refund API was down"), {
      timeoutMs: 5_000,
    });

    assert.equal(server.state.ackCalls[0].applied, false);
    assert.equal(server.state.ackCalls[0].reason, "the refund API was down");
    assert.deepEqual(await waiter.signals(), []);
  });
});

test("SignalNotApplied thrown deep in a call stack is recorded, not propagated", async () => {
  await withServer(async (server) => {
    const client = agent(server);
    const pending = await raiseOne(client, "run:not-applied");
    await person(server).answer(pending.id, { decision: "approve" });

    const applyDeeply = () => {
      throw new SignalNotApplied("downstream ledger rejected the entry");
    };

    const result = await client.waiter(pending.waiterRef).receive(async () => applyDeeply(), {
      timeoutMs: 5_000,
    });

    assert.equal(result, undefined);
    assert.equal(server.state.ackCalls[0].applied, false);
    assert.equal(server.state.ackCalls[0].reason, "downstream ledger rejected the entry");
  });
});

test("a long poll returns as soon as the answer lands", async () => {
  await withServer(async (server) => {
    const client = agent(server);
    const pending = await raiseOne(client, "run:long-poll");

    setTimeout(() => {
      void person(server).answer(pending.id, { decision: "approve" });
    }, 400);

    const started = Date.now();
    const signal = await pending.wait({ timeoutMs: 20_000 });
    const elapsed = Date.now() - started;

    assert.equal(signal.type, "answered");
    assert.ok(elapsed < 10_000, `satisfied by the answer, not the window (${elapsed}ms)`);
  });
});

test("an unmatched signal is left in the queue", async () => {
  // A waiter may hold signals for several requests. Retiring one the caller did not ask about would
  // lose it, so a filtered-out signal is never acked and never dropped.
  await withServer(async (server) => {
    const client = agent(server);
    const first = await raiseOne(client, "run:two-requests");
    const second = await raiseOne(client, "run:two-requests");
    await person(server).answer(first.id, { decision: "approve" });
    await person(server).answer(second.id, { decision: "reject" });

    const waiter = client.waiter("run:two-requests");
    const signal = await waiter.next({
      timeoutMs: 5_000,
      accept: (s) => s.requestId === second.id,
    });
    assert.equal(signal.requestId, second.id);
    assert.deepEqual(server.state.ackCalls, []);

    const stillThere = (await client.waiter("run:two-requests").signals()).map((s) => s.requestId);
    assert.ok(stillThere.includes(first.id));
  });
});

test("attempt_lapsed is a nudge and the terminal signal still arrives", async () => {
  // W2: a non-terminal nudge must never overwrite or mask a later terminal signal, because signals
  // are a queue and not a mutable field.
  await withServer(async (server) => {
    const client = agent(server);
    const pending = await raiseOne(client, "run:nudge");
    server.state.enqueue(pending.id, "attempt_lapsed", null);
    await person(server).answer(pending.id, { decision: "approve" });

    const nudges: any[] = [];
    const signal = await pending.wait({ timeoutMs: 5_000, onAttemptLapsed: (s) => nudges.push(s) });

    assert.deepEqual(nudges.map((n) => n.type), ["attempt_lapsed"]);
    assert.equal(signal.type, "answered");
    assert.ok(signal.sequence > nudges[0].sequence);
    assert.equal(pending.waiter.highestSequence, signal.sequence);
  });
});

test("redemption is idempotent per effect key", async () => {
  // C-13: one answer, one authorization, one effect. A retried turn must not refund twice.
  await withServer(async (server) => {
    const client = agent(server);
    const pending = await raiseOne(client, "run:redeem");
    await person(server).answer(pending.id, { decision: "approve" });

    const authorizationId = await pending.receive(async (received) => {
      const first = await received.outcome.redeem("stripe:refund:ch_1B");
      const second = await received.outcome.redeem("stripe:refund:ch_1B");
      assert.equal(first.firstRedemption, true);
      assert.equal(second.firstRedemption, false);
      return received.outcome.authorizationId;
    });

    assert.ok(authorizationId);
    await assert.rejects(
      () => client.redeem(authorizationId!, "stripe:refund:ch_DIFFERENT"),
      (error: Error) => error instanceof AuthorizationSpent,
    );
  });
});

test("a local timeout is not a protocol outcome", async () => {
  // A caller's deadline passing says nothing about the request, which is still pending and still
  // answerable. The error says so and reattaching proves it.
  await withServer(async (server) => {
    const client = agent(server);
    const pending = await raiseOne(client, "run:timeout");

    await assert.rejects(
      () => pending.wait({ timeoutMs: 1_000 }),
      (error: Error) => {
        assert.ok(error instanceof HandoffTimeout);
        assert.match(error.message, /durable wait is unaffected/);
        return true;
      },
    );

    await person(server).answer(pending.id, { decision: "approve" });
    const resumed = await client.resume(pending.waiterRef);
    assert.equal((await resumed.next({ timeoutMs: 5_000 })).type, "answered");
  });
});

test("the client never prints its api key", async () => {
  const client = new Client({ baseUrl: "https://handoff.example.com", apiKey: "sk_live_do_not_print_me" });
  assert.ok(!String(client).includes("sk_live"));
  assert.ok(String(client).includes("<redacted>"));
});

test("a raised body carries no kind anywhere", async () => {
  // I14 and C-22: the shorthands are constructors, not kinds. The server cannot tell which one the
  // caller used, because nothing on the wire says.
  const { fields, prompt, requires, authority } = await import("../src/index.ts");
  await withServer(async (server) => {
    await agent(server).raiseRequest({
      waiterRef: "run:decl",
      prompt: prompt("Refund $2,400?", "Double charged."),
      requires: requires([fields.choice("decision", "Decision", ["approve", "reject"])], {
        authority: authority("editor", "session"),
      }),
    });
    const raised = [...server.state.requests.values()][0];
    const wire = JSON.stringify({ prompt: raised.prompt, requires: raised.requires });
    assert.ok(!wire.includes('"kind"'));
  });
});

test("ask returns a declared default rather than guessing one later", async () => {
  // §6.4: the default is declared at raise time so the server can mint a policy receipt. Returning
  // it locally on a deadline is the fallback, and it is documented as producing no record.
  await withServer(async (server) => {
    const client = agent(server);
    const answer = await client.ask("Which address?", { default: "the billing one", timeoutMs: 1_000 });
    assert.equal(answer, "the billing one");

    const raised = [...server.state.requests.values()][0];
    assert.equal(raised.requires.answer.fields[0].type, "text");
  });
});

test("approve is truthy only when a person approved", async () => {
  for (const [choice, expected] of [
    ["approve", true],
    ["reject", false],
  ] as const) {
    await withServer(async (server) => {
      const client = agent(server);
      setTimeout(() => {
        const requestId = [...server.state.requests.keys()][0];
        if (requestId) void person(server).answer(requestId, { decision: choice });
      }, 300);
      const outcome = await client.approve("Refund $2,400?", { timeoutMs: 10_000 });
      assert.equal(outcome.approved, expected);
      assert.equal(outcome.decidedByHuman, true);
    });
  }
});
