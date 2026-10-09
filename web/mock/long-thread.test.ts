import type { AddressInfo } from "node:net";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { connect, frames } from "./agui-client";
import { createMockServer } from "./server";

/*
 * `POST /__mock/long-thread`: a finished thread of many turns made at once, which the web's specs open to measure
 * what a long conversation costs (e2e/open-long-thread.spec.ts).
 */

const server = createMockServer({ stepMs: 5, keepaliveMs: 50 });
let base = "";
beforeAll(async () => {
  await new Promise<void>((r) => server.listen(0, "127.0.0.1", r));
  // nosemgrep: opt.opengrep-rules.typescript.react.security.react-insecure-request -- loopback test server, never leaves the runner
  base = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
});
afterAll(async () => {
  server.closeAllConnections();
  await new Promise<void>((r) => server.close(() => r()));
});

const seed = (turns?: number) =>
  fetch(`${base}/__mock/long-thread${turns === undefined ? "" : `?turns=${turns}`}`, {
    method: "POST",
  });

describe("the long thread the mock makes", () => {
  it("is a finished thread of the turns asked for, each one run of the log", async () => {
    const res = await seed(40);
    expect(res.status).toBe(201);
    const made = (await res.json()) as { threadId: string; turns: number; lastSeq: number };
    expect(made.turns).toBe(40);

    const thread = (await (await fetch(`${base}/api/threads/${made.threadId}`)).json()) as {
      state: string;
      lastSeq: number;
      target: { agentId: string };
    };
    expect(thread.state).toBe("done");
    expect(thread.lastSeq).toBe(made.lastSeq);
    expect(thread.target.agentId).toBe("adam");

    const controller = new AbortController();
    const res2 = await connect(base, made.threadId, { signal: controller.signal });
    const seen = await frames(res2, (f) => f.id === made.lastSeq);
    controller.abort();
    const started = seen.filter((f) => f.event.type === "RUN_STARTED");
    const finished = seen.filter((f) => f.event.type === "RUN_FINISHED");
    expect(started).toHaveLength(40);
    expect(finished).toHaveLength(40);
    // a person's message per turn, each with its own words
    const asked = seen.filter(
      (f) => f.event.type === "TEXT_MESSAGE_CONTENT" && String(f.event.delta).startsWith("Turn "),
    );
    expect(asked.map((f) => f.event.delta)).toContain(
      "Turn 36: Open a pull request for the change.",
    );
    expect(asked).toHaveLength(40);
  });

  it("takes 200 turns when it is not told how many", async () => {
    const made = (await (await seed()).json()) as { turns: number };
    expect(made.turns).toBe(200);
  });

  it.each([0, 2001, 1.5])("refuses %s turns", async (turns) => {
    const res = await seed(turns);
    expect(res.status).toBe(400);
  });
});
