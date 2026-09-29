import { readdirSync, readFileSync } from "node:fs";
import type { AddressInfo } from "node:net";
import path from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import type { components } from "../src/lib/api/schema";
import { createMockServer } from "./server";

/**
 * The mock must tell the same story as the real orchestrator. Each golden transcript
 * (docs/api/examples, written by orchestrator/crates/e2e/tests/golden.rs) is replayed against
 * the mock with the same messages; the event kinds, actor types and `data` must be equal.
 * Only what is legitimately different is normalised: ids, timestamps, the user's and the
 * agent's names (the mock's `reviewer` plays the orchestrator test's `plain`; both
 * have no releases).
 */

type Event = components["schemas"]["Event"];
type Thread = components["schemas"]["Thread"];

const DIR = path.resolve(import.meta.dirname, "../../docs/api/examples");

const server = createMockServer({ stepMs: 2, keepaliveMs: 1000 });
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

const call = (p: string, method: string, body?: unknown) =>
  fetch(base + p, {
    method,
    headers: { "Content-Type": "application/json" },
    ...(body === undefined ? {} : { body: JSON.stringify(body) }),
  });

async function thread(id: string): Promise<Thread> {
  return (await (await call(`/api/threads/${id}`, "GET")).json()) as Thread;
}

async function waitForState(id: string, state: Thread["state"]) {
  for (let i = 0; i < 500; i++) {
    if ((await thread(id)).state === state) return;
    await new Promise((r) => setTimeout(r, 5));
  }
  throw new Error(`thread ${id} never reached ${state}`);
}

/** The scenarios of golden.rs, driven through the mock's API. Returns the final state. */
const SCENARIOS: Record<
  string,
  (
    create: (text: string, target?: object) => Promise<string>,
  ) => Promise<{ id: string; last: Thread["state"] }>
> = {
  echo: async (create) => ({ id: await create("echo hi"), last: "done" }),
  ask: async (create) => {
    const id = await create("ask about branches");
    await waitForState(id, "blocked");
    expect((await call(`/api/threads/${id}/messages`, "POST", { text: "main" })).status).toBe(202);
    return { id, last: "done" };
  },
  cancel: async (create) => {
    const id = await create("slow work");
    await waitForState(id, "working");
    expect((await call(`/api/threads/${id}/cancel`, "POST")).status).toBe(202);
    return { id, last: "cancelled" };
  },
  fail: async (create) => ({ id: await create("fail please"), last: "failed" }),
  talk: async (create) => ({ id: await create("talk to me"), last: "done" }),
  release: async (create) => ({
    id: await create("echo ship it", { agentId: "coder", release: "staging" }),
    last: "done",
  }),
};

/** Kind, actor type, revision and data with the ids removed. */
function story(events: Pick<Event, "kind" | "actor" | "data">[]) {
  return events.map((e) => ({
    kind: e.kind,
    actor: e.actor.type,
    revision: e.actor.revision,
    data: e.kind === "agent_message" ? { ...e.data, messageId: "<message-id>" } : e.data,
  }));
}

describe("the mock server against the golden transcripts", () => {
  it("has a scenario for every golden transcript", () => {
    const files = readdirSync(DIR)
      .filter((f) => f.endsWith(".events.json"))
      .map((f) => f.replace(/\.events\.json$/, ""));
    expect(files.sort()).toEqual(Object.keys(SCENARIOS).sort());
  });

  for (const [name, run] of Object.entries(SCENARIOS)) {
    it(`the mock tells the same story as the orchestrator: ${name}`, async () => {
      const golden = JSON.parse(
        readFileSync(path.join(DIR, `${name}.events.json`), "utf8"),
      ) as Event[];
      const create = async (text: string, target: object = { agentId: "reviewer" }) => {
        const res = await call("/api/threads", "POST", { target, text });
        expect(res.status).toBe(201);
        return ((await res.json()) as Thread).id;
      };
      const { id, last } = await run(create);
      await waitForState(id, last);
      const events = (await (await call(`/api/threads/${id}/events`, "GET")).json()) as Event[];
      expect(story(events)).toEqual(story(golden));
    });
  }
});
