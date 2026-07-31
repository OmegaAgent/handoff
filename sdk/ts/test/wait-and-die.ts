/**
 * A process whose only job is to block in a long poll and then be killed.
 *
 * Spawned by `reattach.test.ts`. It prints `polling` once the wait is in flight so the parent knows
 * when it is safe to send SIGKILL, and should never reach the line after it.
 */

import { Client } from "../src/index.ts";

const [baseUrl, waiterRef] = process.argv.slice(2);

const client = new Client({ baseUrl, apiKey: "test-key" });
console.log("polling");
await client.waiter(waiterRef).next({ timeoutMs: 60_000 });
console.log("SHOULD NOT REACH");
