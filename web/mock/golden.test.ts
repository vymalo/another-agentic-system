import { readdirSync, readFileSync } from "node:fs";
import type { AddressInfo } from "node:net";
import path from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import type { components } from "../src/lib/api/schema";
import { connect, type Frame, frames, postRun, RELEASE_CHANNELS_URI } from "./agui-client";
import { createMockServer } from "./server";

/**
 * The mock must tell the same story as the real orchestrator. Each scenario of golden.rs is
 * driven through the mock's AG-UI routes, and the connect stream a viewer reads must be the
 * golden of docs/api/examples/agui (`<name>.agui.json`, written by orch-agui-projection from the
 * golden event log). The ids a client chooses are the ones the golden has (`evt-1`, `run-5`); what
 * is legitimately different is normalised: the thread id, the user's and the agent's names (the
 * mock's `reviewer` plays the orchestrator test's `plain`) and an agent message's id.
 */

type Thread = components["schemas"]["Thread"];

const DIR = path.resolve(import.meta.dirname, "../../docs/api/examples/agui");
/**
 * Goldens the mock does not play yet: `a2ui` is the orchestrator's A2UI story (a surface, an
 * action back through `forwardedProps.a2uiAction`); the renderer that needs it, and the mock's
 * part in it, come with the web's A2UI slice.
 */
const NOT_PLAYED = ["a2ui"];

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

async function thread(id: string): Promise<Thread> {
  return (await (await fetch(`${base}/api/threads/${id}`)).json()) as Thread;
}

async function waitForState(id: string, state: Thread["state"]) {
  for (let i = 0; i < 500; i++) {
    if ((await thread(id)).state === state) return;
    await new Promise((r) => setTimeout(r, 5));
  }
  throw new Error(`thread ${id} never reached ${state}`);
}

let n = 0;
const newThreadId = () => `00000000-0000-4000-8000-${String(++n).padStart(12, "0")}`;

/** The scenarios of golden.rs, driven through the mock's AG-UI run route. Returns the final state. */
const SCENARIOS: Record<string, (id: string) => Promise<{ agent: string; last: Thread["state"] }>> =
  {
    echo: async (id) => {
      const res = await postRun(base, "reviewer", {
        threadId: id,
        runId: "run-1",
        messages: [{ id: "evt-1", role: "user", content: "echo hi" }],
      });
      expect(res.status).toBe(200);
      return { agent: "reviewer", last: "done" };
    },
    ask: async (id) => {
      const first = await postRun(base, "reviewer", {
        threadId: id,
        runId: "run-1",
        messages: [{ id: "evt-1", role: "user", content: "ask about branches" }],
      });
      expect(first.status).toBe(200);
      await waitForState(id, "blocked");
      const answer = await postRun(base, "reviewer", {
        threadId: id,
        runId: "run-5",
        messages: [],
        resume: [{ interruptId: "int-3", status: "resolved", payload: { text: "main" } }],
      });
      expect(answer.status).toBe(200);
      return { agent: "reviewer", last: "done" };
    },
    cancel: async (id) => {
      const res = await postRun(base, "reviewer", {
        threadId: id,
        runId: "run-1",
        messages: [{ id: "evt-1", role: "user", content: "slow work" }],
      });
      expect(res.status).toBe(200);
      await waitForState(id, "working");
      expect((await fetch(`${base}/api/threads/${id}/cancel`, { method: "POST" })).status).toBe(
        202,
      );
      return { agent: "reviewer", last: "cancelled" };
    },
    fail: async (id) => {
      const res = await postRun(base, "reviewer", {
        threadId: id,
        runId: "run-1",
        messages: [{ id: "evt-1", role: "user", content: "fail please" }],
      });
      expect(res.status).toBe(200);
      return { agent: "reviewer", last: "failed" };
    },
    talk: async (id) => {
      const res = await postRun(base, "reviewer", {
        threadId: id,
        runId: "run-1",
        messages: [{ id: "evt-1", role: "user", content: "talk to me" }],
      });
      expect(res.status).toBe(200);
      return { agent: "reviewer", last: "done" };
    },
    release: async (id) => {
      const res = await postRun(base, "coder", {
        threadId: id,
        runId: "run-1",
        messages: [{ id: "evt-1", role: "user", content: "echo ship it" }],
        forwardedProps: { [RELEASE_CHANNELS_URI]: { release: "staging" } },
      });
      expect(res.status).toBe(200);
      return { agent: "coder", last: "done" };
    },
  };

/** Names and ids that legitimately differ between the mock and the golden. */
function normalise(list: Frame[], threadId: string): Frame[] {
  const text = JSON.stringify(list)
    .replaceAll(threadId, "<thread-id>")
    .replaceAll("dev@example.com", "alice@example.com")
    .replaceAll('"reviewer"', '"plain"');
  const out = JSON.parse(text) as Frame[];
  // the mock names agent messages m-<n>; the golden msg-<seq of the END frame>
  const seqOf = new Map<string, number>();
  for (const f of out) {
    const id = f.event.messageId;
    if (f.event.type === "TEXT_MESSAGE_END" && typeof id === "string" && id.startsWith("m-")) {
      seqOf.set(id, f.id ?? 0);
    }
  }
  return JSON.parse(
    JSON.stringify(out).replace(/"m-\d+"/g, (m) => `"msg-${seqOf.get(JSON.parse(m) as string)}"`),
  ) as Frame[];
}

describe("the mock server against the AG-UI goldens", () => {
  it("has a scenario for every golden event log", () => {
    const files = readdirSync(path.join(DIR, ".."))
      .filter((f) => f.endsWith(".events.json"))
      .map((f) => f.replace(/\.events\.json$/, ""))
      .filter((name) => !NOT_PLAYED.includes(name));
    expect(files.sort()).toEqual(Object.keys(SCENARIOS).sort());
  });

  for (const [name, run] of Object.entries(SCENARIOS)) {
    it(`a viewer reads the golden stream: ${name}`, async () => {
      const golden = JSON.parse(
        readFileSync(path.join(DIR, `${name}.agui.json`), "utf8"),
      ) as Frame[];
      const id = newThreadId();
      const { last } = await run(id);
      await waitForState(id, last);
      const viewer = await frames(await connect(base, id, { mode: "run" }));
      expect(normalise(viewer, id)).toEqual(golden);
    });
  }

  it("a viewer that reconnects with Last-Event-ID is sent the preamble and the rest", async () => {
    const id = newThreadId();
    await postRun(base, "reviewer", {
      threadId: id,
      runId: "run-1",
      messages: [{ id: "evt-1", role: "user", content: "echo hi" }],
    });
    await waitForState(id, "done");
    const all = await frames(await connect(base, id, { mode: "run" }));
    // held the first two log events; the rest is the run's second half, under the same run
    const held = all.filter((f) => f.id !== undefined && f.id <= 2);
    const rest = await frames(await connect(base, id, { mode: "run", lastEventId: 2 }));
    expect(rest.slice(0, 3).map((f) => f.event.type)).toEqual([
      "RUN_STARTED",
      "SUBAGENT_STARTED",
      "STATE_SNAPSHOT",
    ]);
    expect(rest[0]?.event.runId).toBe("run-1");
    expect(rest.slice(0, 3).every((f) => f.id === undefined)).toBe(true);
    // what the client held (up to the frame with id 2) plus what came after the preamble is the stream
    const tail = rest.slice(3);
    const heldFrames = all.slice(0, all.findIndex((f) => f.id === 2) + 1);
    expect(held.length).toBe(2);
    expect([...heldFrames, ...tail]).toEqual(all);
  });
});
