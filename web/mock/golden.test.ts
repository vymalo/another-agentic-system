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
 * mock's `reviewer` plays the orchestrator test's `plain`), an agent message's id and the time of
 * an activity (`at`: the mock's clock is real, the golden's is fixed; both must have one).
 */

type Thread = components["schemas"]["Thread"];
type Event = components["schemas"]["Event"];

const DIR = path.resolve(import.meta.dirname, "../../docs/api/examples/agui");
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

/**
 * The two catalogs of the `catalog` scenario, as the orchestrator's golden recorded them (the
 * `ui_catalog` events of catalog.events.json: version 1, then 2): the digests the stream names
 * are theirs, and the mock recomputes them, so the two canonical JSONs agree on these catalogs too.
 */
function goldenCatalogs(): Record<string, unknown>[] {
  const log = JSON.parse(
    readFileSync(path.join(DIR, "..", "catalog.events.json"), "utf8"),
  ) as Event[];
  return log.filter((e) => e.kind === "ui_catalog").map((e) => e.data as Record<string, unknown>);
}

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
    // a surface, the question, and the owner's action on the surface (not a message, not a resume)
    a2ui: async (id) => {
      const first = await postRun(base, "reviewer", {
        threadId: id,
        runId: "run-1",
        messages: [{ id: "evt-1", role: "user", content: "ui pick one" }],
      });
      expect(first.status).toBe(200);
      await waitForState(id, "blocked");
      const action = await postRun(base, "reviewer", {
        threadId: id,
        runId: "run-2",
        messages: [],
        forwardedProps: {
          a2uiAction: {
            userAction: {
              name: "go",
              surfaceId: "s1",
              sourceComponentId: "go",
              context: { choice: "a" },
            },
          },
        },
      });
      expect(action.status).toBe(200);
      return { agent: "reviewer", last: "done" };
    },
    // the verification gate (ADR 0018): red, sent back, green on attempt 2 of 3
    "verify-green": async (id) => {
      const res = await postRun(base, "reviewer", {
        threadId: id,
        runId: "run-1",
        messages: [{ id: "evt-1", role: "user", content: "verify-red-once fix the login" }],
      });
      expect(res.status).toBe(200);
      return { agent: "reviewer", last: "done" };
    },
    // three attempts whose checks all fail: the run ends in RUN_ERROR checks_failed
    "verify-red": async (id) => {
      const res = await postRun(base, "reviewer", {
        threadId: id,
        runId: "run-1",
        messages: [{ id: "evt-1", role: "user", content: "verify-red fix the login" }],
      });
      expect(res.status).toBe(200);
      return { agent: "reviewer", last: "failed" };
    },
    // the verifier agent of the gate (ADR 0018, MVP slice 10): findings once, then a pass; the
    // verifier is a subagent of its own. Both goldens have the same first message, which the mock
    // cannot tell apart, so the red one is spelled `verify-reviewed-red` (normalise puts it back)
    "verify-verifier-green": async (id) => {
      const res = await postRun(base, "reviewer", {
        threadId: id,
        runId: "run-1",
        messages: [{ id: "evt-1", role: "user", content: "verify-reviewed fix the login" }],
      });
      expect(res.status).toBe(200);
      return { agent: "reviewer", last: "done" };
    },
    // three attempts the verifier never passes: the run ends in RUN_ERROR checks_failed
    "verify-verifier-red": async (id) => {
      const res = await postRun(base, "reviewer", {
        threadId: id,
        runId: "run-1",
        messages: [{ id: "evt-1", role: "user", content: "verify-reviewed-red fix the login" }],
      });
      expect(res.status).toBe(200);
      return { agent: "reviewer", last: "failed" };
    },
    // CI on the pushed commit (ADR 0017): a red `ci/build`, the agent sent back, a green one
    ci: async (id) => {
      const res = await postRun(base, "reviewer", {
        threadId: id,
        runId: "run-1",
        messages: [{ id: "evt-1", role: "user", content: "verify-ci fix the login" }],
      });
      expect(res.status).toBe(200);
      return { agent: "reviewer", last: "done" };
    },
    // a thread is a conversation (ADR 0020): a message on a finished thread starts its next job
    followup: async (id) => {
      const first = await postRun(base, "reviewer", {
        threadId: id,
        runId: "run-1",
        messages: [{ id: "evt-1", role: "user", content: "echo hi" }],
      });
      expect(first.status).toBe(200);
      await waitForState(id, "done");
      const second = await postRun(base, "reviewer", {
        threadId: id,
        runId: "run-6",
        messages: [{ id: "evt-6", role: "user", content: "echo now add tests" }],
      });
      expect(second.status).toBe(200);
      return { agent: "reviewer", last: "done" };
    },
    // a stopped thread is not closed either
    "followup-after-cancel": async (id) => {
      const first = await postRun(base, "reviewer", {
        threadId: id,
        runId: "run-1",
        messages: [{ id: "evt-1", role: "user", content: "slow work" }],
      });
      expect(first.status).toBe(200);
      await waitForState(id, "working");
      expect((await fetch(`${base}/api/threads/${id}/cancel`, { method: "POST" })).status).toBe(
        202,
      );
      await waitForState(id, "cancelled");
      const second = await postRun(base, "reviewer", {
        threadId: id,
        runId: "run-5",
        messages: [{ id: "evt-5", role: "user", content: "echo never mind, do this" }],
      });
      expect(second.status).toBe(200);
      return { agent: "reviewer", last: "done" };
    },
    // the UI's catalog (ADR 0023): the first run carries version 1, the next job version 2, the
    // third job version 1 again (an older screen): recorded once each, current stays 2
    catalog: async (id) => {
      const [v1, v2] = goldenCatalogs();
      const run = async (n: number, text: string, catalog: unknown) => {
        const res = await postRun(base, "reviewer", {
          threadId: id,
          runId: `run-${n}`,
          messages: [{ id: `msg-${n}`, role: "user", content: text }],
          forwardedProps: { "vymalo.uiCatalog": catalog },
        });
        expect(res.status).toBe(200);
        await waitForState(id, "done");
      };
      await run(1, "echo hi", v1);
      await run(2, "echo again", v2);
      await run(3, "echo once more", v1);
      return { agent: "reviewer", last: "done" };
    },
    // nested steps (ADR 0025, MVP slice 5): a sub-agent step with a command under it
    steps: async (id) => {
      const res = await postRun(base, "reviewer", {
        threadId: id,
        runId: "run-1",
        messages: [{ id: "evt-1", role: "user", content: "steps run the tests" }],
      });
      expect(res.status).toBe(200);
      return { agent: "reviewer", last: "done" };
    },
    // a step that is waiting when the agent asks: its subagent suspends with the invocation
    "steps-ask": async (id) => {
      const first = await postRun(base, "reviewer", {
        threadId: id,
        runId: "run-1",
        messages: [{ id: "evt-1", role: "user", content: "steps-ask clean the build" }],
      });
      expect(first.status).toBe(200);
      await waitForState(id, "blocked");
      const answer = await postRun(base, "reviewer", {
        threadId: id,
        runId: "run-7",
        messages: [],
        resume: [{ interruptId: "int-5", status: "resolved", payload: { text: "yes" } }],
      });
      expect(answer.status).toBe(200);
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

/**
 * Every activity's `at` becomes `<timestamp>`; one without it fails the comparison. So does a
 * step's `startedAt` (a `vymalo.step` has both).
 */
function untimed(list: Frame[]): Frame[] {
  return list.map((f) => {
    const content = f.event.content;
    if (f.event.type !== "ACTIVITY_SNAPSHOT" || typeof content !== "object" || content === null) {
      return f;
    }
    const at = (content as Record<string, unknown>).at;
    if (at === undefined) return f;
    expect(typeof at).toBe("string");
    const timed: Record<string, unknown> = { ...content, at: "<timestamp>" };
    if (f.event.activityType === "vymalo.step") {
      expect(typeof timed.startedAt).toBe("string");
      timed.startedAt = "<timestamp>";
    }
    return { ...f, event: { ...f.event, content: timed } };
  });
}

/** Names and ids that legitimately differ between the mock and the golden. */
function normalise(list: Frame[], threadId: string): Frame[] {
  const text = JSON.stringify(list)
    .replaceAll(threadId, "<thread-id>")
    .replaceAll("dev@example.com", "alice@example.com")
    .replaceAll('"reviewer"', '"plain"')
    // the verifier agent of the mock is `verifier`, the golden's is named `reviewer`
    .replaceAll('"name":"verifier"', '"name":"reviewer"')
    .replaceAll("verify-reviewed-red fix", "verify-reviewed fix");
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
      .map((f) => f.replace(/\.events\.json$/, ""));
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
      expect(untimed(normalise(viewer, id))).toEqual(untimed(golden));
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
