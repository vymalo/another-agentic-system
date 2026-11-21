import type { AddressInfo } from "node:net";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { connect, frames, postRun } from "./agui-client";
import { createMockServer } from "./server";

/**
 * The mock's stream lifetime (ADR 0082, decision 1): a followed stream ends on its own terms after the
 * deployment's lifetime, and a client that reopens with its last `id:` is shown every later event once.
 * The mock has to behave that way, or the web's tests would prove a reopen the orchestrator never makes.
 */
describe("a followed stream ends at the lifetime, and the reopen continues it", () => {
  // short, so the test does not wait a minute
  const server = createMockServer({ stepMs: 5, keepaliveMs: 50, lifetimeMs: 150 });
  let base = "";

  beforeAll(async () => {
    await new Promise<void>((r) => server.listen(0, "127.0.0.1", r));
    base = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
  });
  afterAll(() => {
    server.closeAllConnections();
    server.close();
  });

  it("ends by itself while the run is still open, and the reopen from the last id says the rest", async () => {
    const threadId = "11111111-1111-4111-8111-111111111111";
    // a run that waits: the stream has something to say, and then nothing at all
    const run = await postRun(base, "reviewer", {
      threadId,
      runId: "run-1",
      messages: [{ id: "evt-1", role: "user", content: "gate hold" }],
    });
    expect(run.status).toBe(200);
    // (the run's own response is the run's: it is not the one the lifetime ends)
    await frames(run, (f) => f.event.type === "RUN_STARTED");

    const first = await connect(base, threadId);
    expect(first.status).toBe(200);
    // the stream ends with no error and no close of its own: the mock ended it, as the orchestrator does
    const before = await frames(first);
    const cursor = before.filter((f) => f.id !== undefined).at(-1)?.id;
    expect(cursor).toBeGreaterThan(0);
    expect(before.some((f) => f.event.type === "RUN_STARTED")).toBe(true);

    // the run goes on after the stream ended, and the reopen from the cursor is told it, once
    const released = await fetch(`${base}/__mock/release?thread=${threadId}`, { method: "POST" });
    expect(released.status).toBe(204);
    await new Promise((r) => setTimeout(r, 100));

    const second = await connect(base, threadId, { lastEventId: cursor });
    const after = await frames(second);
    expect(after.some((f) => f.event.type === "RUN_FINISHED")).toBe(true);
    // nothing of what the first stream said is said again
    for (const f of after) if (f.id !== undefined) expect(f.id).toBeGreaterThan(cursor as number);
  });
});
