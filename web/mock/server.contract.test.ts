import { readFileSync } from "node:fs";
import type { AddressInfo } from "node:net";
import path from "node:path";
import Ajv2020 from "ajv/dist/2020.js";
import addFormats from "ajv-formats";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { parse } from "yaml";
import { OWN_CATALOG, UI_CATALOG_PROP } from "../src/features/chat/lib/a2ui/catalog";
import { catalogDigest } from "../src/features/chat/lib/a2ui/catalog/digest";
import { readSse, type SseFrame } from "../src/features/chat/lib/agui/sse";
import type { components } from "../src/lib/api/schema";
import {
  connect,
  type Frame,
  frames,
  isTerminal,
  postRun,
  RELEASE_CHANNELS_URI,
} from "./agui-client";
import { createMockServer } from "./server";

/**
 * Keeps the mock honest: every response it produces (JSON bodies and SSE frames) is validated
 * against the schemas in docs/api/chat-api.yaml.
 */

type Thread = components["schemas"]["Thread"];

const contract = parse(
  readFileSync(path.resolve(import.meta.dirname, "../../docs/api/chat-api.yaml"), "utf8"),
) as {
  paths: Record<string, Record<string, { responses: Record<string, unknown> }>>;
  components: Record<string, unknown>;
};

// CJS interop: depending on the loader, the default import is the class or the module object.
const interop = <T>(m: T): T => (m as unknown as { default?: T }).default ?? m;
const ajv = new (interop(Ajv2020))({ strict: false, allErrors: true });
interop(addFormats)(ajv);
ajv.addFormat("int64", true);
// The AG-UI schemas of the contract are references to the vendored JSON Schema, by file.
const AGUI_SCHEMA_REF = "../../orchestrator/crates/agui-proto/schema/ag-ui-1.0.schema.json";
const aguiSchema = JSON.parse(
  readFileSync(
    path.resolve(
      import.meta.dirname,
      "../../orchestrator/crates/agui-proto/schema/ag-ui-1.0.schema.json",
    ),
    "utf8",
  ),
) as { $id: string };
ajv.addSchema(aguiSchema);

const rewriteRefs = (schema: unknown) =>
  JSON.parse(
    JSON.stringify(schema)
      .replaceAll('"#/components', '"chat#/components')
      .replaceAll(`"${AGUI_SCHEMA_REF}#`, `"${aguiSchema.$id}#`),
  );
ajv.addSchema({ $id: "chat", components: rewriteRefs(contract.components) });

function resolveRef<T>(node: unknown): T {
  const ref = (node as { $ref?: string }).$ref;
  if (!ref) return node as T;
  let cur: unknown = { components: contract.components };
  for (const part of ref.replace(/^#\//, "").split("/"))
    cur = (cur as Record<string, unknown>)[part];
  return cur as T;
}

function validateAgainst(schema: unknown, body: unknown, label: string) {
  const validate = ajv.compile(rewriteRefs(schema));
  const ok = validate(body);
  expect(ok, `${label}: ${JSON.stringify(validate.errors)} in ${JSON.stringify(body)}`).toBe(true);
}

/** Assert `res` is a documented response of `method template` and its body matches the schema. */
async function expectDocumented(template: string, method: string, res: Response): Promise<unknown> {
  const op = contract.paths[template]?.[method];
  expect(op, `${method} ${template} is in the contract`).toBeDefined();
  const def = op?.responses[String(res.status)];
  expect(def, `${method} ${template} documents ${res.status}`).toBeDefined();
  const resolved = resolveRef<{ content?: Record<string, { schema: unknown }> }>(def);
  if (!resolved.content) return undefined;
  const contentType = (res.headers.get("content-type") ?? "").split(";")[0] ?? "";
  const media = resolved.content[contentType];
  expect(media, `content-type ${contentType} is documented`).toBeDefined();
  if (contentType === "text/event-stream") return undefined;
  const body: unknown = await res.json();
  validateAgainst(media?.schema, body, `${method} ${template} ${res.status}`);
  return body;
}

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

const post = (p: string, body?: unknown) =>
  fetch(base + p, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    ...(body === undefined ? {} : { body: JSON.stringify(body) }),
  });

const patch = (p: string, body?: unknown) =>
  fetch(base + p, {
    method: "PATCH",
    headers: { "Content-Type": "application/json" },
    ...(body === undefined ? {} : { body: JSON.stringify(body) }),
  });

let counter = 0;
const newId = () => `00000000-0000-4000-8000-${String(++counter).padStart(12, "0")}`;

/** A run's frames, each validated against the vendored AG-UI schema (`AgUiEvent` of the contract). */
async function validated(list: Frame[], label: string): Promise<Frame[]> {
  for (const f of list) validateAgainst({ $ref: "#/components/schemas/AgUiEvent" }, f.event, label);
  return list;
}

async function startThread(
  text: string,
  agent = "adam",
  extra: { forwardedProps?: Record<string, unknown> } = {},
) {
  // A run that waits (slow, verify-wait, verify-reviewed-wait) never ends its response: read up to RUN_STARTED.
  const untilStarted =
    text.startsWith("slow") ||
    text.startsWith("verify-wait") ||
    text.startsWith("verify-reviewed-wait") ||
    text.startsWith("ask-hold");
  const threadId = newId();
  const res = await postRun(base, agent, {
    threadId,
    runId: "run-1",
    messages: [{ id: "m-1", role: "user", content: text }],
    ...extra,
  });
  expect(res.status).toBe(200);
  const body = await validated(
    await frames(res, untilStarted ? (f) => f.event.type === "RUN_STARTED" : undefined),
    "run frames",
  );
  return { threadId, body };
}

async function waitForState(id: string, states: Thread["state"][]): Promise<Thread> {
  for (let i = 0; i < 200; i++) {
    const res = await fetch(`${base}/api/threads/${id}`);
    const t = (await expectDocumented("/api/threads/{threadId}", "get", res)) as Thread;
    if (states.includes(t.state)) return t;
    await new Promise((r) => setTimeout(r, 10));
  }
  throw new Error(`thread ${id} never reached ${states.join("|")}`);
}

const types = (list: Frame[]) => list.map((f) => f.event.type);

describe("mock server honours docs/api/chat-api.yaml", () => {
  it("lists agents; only adam advertises releases", async () => {
    const res = await fetch(`${base}/api/agents`);
    const agents = (await expectDocumented(
      "/api/agents",
      "get",
      res,
    )) as components["schemas"]["Agent"][];
    expect(agents.find((a) => a.id === "adam")?.releases?.defaultChannel).toBe("production");
    expect(agents.find((a) => a.id === "reviewer")?.releases).toBeUndefined();
  });

  it("an agent answers to its aliases, and is listed and creates threads under its id only (ADR 0049)", async () => {
    const res = await fetch(`${base}/api/agents`);
    const agents = (await expectDocumented(
      "/api/agents",
      "get",
      res,
    )) as components["schemas"]["Agent"][];
    expect(agents.map((a) => a.id)).not.toContain("coder");
    expect(agents.find((a) => a.id === "adam")?.aliases).toEqual(["coder"]);
    expect(agents.find((a) => a.id === "reviewer")).not.toHaveProperty("aliases");
    // a run on the alias is a run of the agent: the thread is created under its id
    const { threadId } = await startThread("Implement the thing", "coder");
    const t = await waitForState(threadId, ["done"]);
    expect(t.target.agentId).toBe("adam");
    const caps = await fetch(`${base}/agui/agents/coder/capabilities`);
    expect(caps.status).toBe(200);
  });

  it("the registry: its agents come after the configured ones, and none while it cannot be read", async () => {
    const list = async () => {
      const res = await fetch(`${base}/api/agents`);
      return (await expectDocumented(
        "/api/agents",
        "get",
        res,
      )) as components["schemas"]["Agent"][];
    };
    const status = async () => {
      const res = await fetch(`${base}/api/registry`);
      return (await expectDocumented("/api/registry", "get", res)) as {
        sources: { name: string; status: string; detail?: string }[];
      };
    };
    try {
      expect((await list()).map((a) => a.source)).toEqual(["static", "static", "static"]);
      expect(await status()).toEqual({
        sources: [
          { name: "static", status: "ok" },
          { name: "platform", status: "ok" },
        ],
      });

      await post("/__mock/registry/agents", { id: "helper", name: "Helper", tags: ["writing"] });
      const agents = await list();
      expect(agents.map((a) => a.id)).toEqual(["adam", "reviewer", "verifier", "helper"]);
      expect(agents[3]).toMatchObject({ source: "registry", tags: ["writing"] });

      // down: none of the registry's agents, and the status says which source and why
      await post("/__mock/registry?down=true");
      expect((await list()).map((a) => a.id)).toEqual(["adam", "reviewer", "verifier"]);
      expect(await status()).toEqual({
        sources: [
          { name: "static", status: "ok" },
          { name: "platform", status: "unavailable", detail: "the registry could not be reached" },
        ],
      });
      // an agent that cannot be said to exist is a 503, never a 404 (and a static one still runs)
      const res = await postRun(base, "helper", {
        threadId: newId(),
        runId: "run-1",
        messages: [{ id: "m-1", role: "user", content: "hi" }],
      });
      expect(res.status).toBe(503);
      expect(res.headers.get("retry-after")).not.toBeNull();
      await expectDocumented("/agui/agents/{agentId}", "post", res);
      const caps = await fetch(`${base}/agui/agents/helper/capabilities`);
      expect(caps.status).toBe(503);
      await expectDocumented("/agui/agents/{agentId}/capabilities", "get", caps);
      const ok = await postRun(base, "reviewer", {
        threadId: newId(),
        runId: "run-1",
        messages: [{ id: "m-1", role: "user", content: "echo hi" }],
      });
      expect(ok.status).toBe(200);
      await ok.body?.cancel();
    } finally {
      await post("/__mock/registry?down=false");
    }
    // up again: back, and reset forgets the registry's agents
    expect((await list()).map((a) => a.id)).toContain("helper");
    await post("/__mock/reset");
    expect((await list()).map((a) => a.id)).toEqual(["adam", "reviewer", "verifier"]);
  });

  it("run route: RUN_STARTED first, the run to its terminal event, then EOF; frames conform", async () => {
    const { threadId, body } = await startThread("Implement the thing");
    expect(body[0]?.event).toMatchObject({ type: "RUN_STARTED", threadId, runId: "run-1" });
    expect(body.at(-1)?.event).toMatchObject({
      type: "RUN_FINISHED",
      outcome: { type: "success" },
    });
    // the requester holds its own message: the user's text is not sent back
    expect(types(body)).not.toContain("TEXT_MESSAGE_START");
    const t = await waitForState(threadId, ["done"]);
    expect(t).toMatchObject({
      lastSeq: 5,
      target: { agentId: "adam" },
      title: "Implement the thing",
    });
  });

  it("connect route: replay, ids on the last frame of each log event, Last-Event-ID resumes", async () => {
    const { threadId } = await startThread("Another one");
    await waitForState(threadId, ["done"]);
    const all = await validated(
      await frames(await connect(base, threadId, { mode: "run" })),
      "connect",
    );
    const ids = all.flatMap((f) => (f.id === undefined ? [] : [f.id]));
    expect(ids).toEqual([1, 2, 3, 4, 5]);
    expect(types(all)[0]).toBe("RUN_STARTED");
    const resumed = await frames(await connect(base, threadId, { mode: "run", lastEventId: 3 }));
    expect(types(resumed).slice(0, 3)).toEqual([
      "RUN_STARTED",
      "SUBAGENT_STARTED",
      "STATE_SNAPSHOT",
    ]);
    expect(
      resumed
        .slice(3)
        .map((f) => f.id)
        .filter((v) => v !== undefined),
    ).toEqual([4, 5]);
    // a cursor at the end of a finished thread: nothing to send, the stream just ends
    expect(await frames(await connect(base, threadId, { mode: "run", lastEventId: 5 }))).toEqual(
      [],
    );
  });

  it("connect route stays open for the next run and streams it live", async () => {
    const { threadId } = await startThread("ask which branch");
    await waitForState(threadId, ["blocked"]);
    const ac = new AbortController();
    const live = frames(await connect(base, threadId, { lastEventId: 4, signal: ac.signal }), (f) =>
      isTerminal(f),
    );
    const answer = await postRun(base, "adam", {
      threadId,
      runId: "run-2",
      messages: [],
      resume: [{ interruptId: "int-3", status: "resolved", payload: { text: "main" } }],
    });
    expect(answer.status).toBe(200);
    const list = await validated(await live, "live");
    ac.abort();
    expect(types(list)[0]).toBe("RUN_STARTED");
    expect(list[0]?.event.runId).toBe("run-2");
    expect(list.at(-1)?.event).toMatchObject({
      type: "RUN_FINISHED",
      outcome: { type: "success" },
    });
  });

  it("uses the selected release for the actor revision", async () => {
    const { threadId, body } = await startThread("Use staging", "adam", {
      forwardedProps: { [RELEASE_CHANNELS_URI]: { release: "staging" } },
    });
    const t = await waitForState(threadId, ["done"]);
    expect(t.target.release).toBe("staging");
    expect(JSON.stringify(body)).toContain('"revision":"coder-r51"');
  });

  it("rejects bad runs with a documented problem", async () => {
    const good = {
      threadId: newId(),
      runId: "r",
      messages: [{ id: "m", role: "user" as const, content: "hi" }],
    };
    const cases: [number, string, Parameters<typeof postRun>[2]][] = [
      [400, "adam", { ...good, threadId: "not-a-uuid" }],
      [404, "nope", good],
      [
        400,
        "reviewer",
        { ...good, forwardedProps: { [RELEASE_CHANNELS_URI]: { release: "staging" } } },
      ],
      [400, "adam", { ...good, forwardedProps: { [RELEASE_CHANNELS_URI]: { release: "nope" } } }],
      [422, "adam", { ...good, messages: [] }],
    ];
    for (const [status, agent, input] of cases) {
      const res = await postRun(base, agent, input);
      expect(res.status, JSON.stringify(input)).toBe(status);
      await expectDocumented("/agui/agents/{agentId}", "post", res);
    }
  });

  it("answers 409 for a run already open, a finished thread and another agent", async () => {
    const slow = await startThread("slow task", "reviewer");
    await waitForState(slow.threadId, ["working"]);
    const open = await postRun(base, "reviewer", {
      threadId: slow.threadId,
      runId: "run-2",
      messages: [{ id: "m-2", role: "user", content: "again" }],
    });
    expect(open.status).toBe(409);
    await expectDocumented("/agui/agents/{agentId}", "post", open);
    const other = await postRun(base, "adam", {
      threadId: slow.threadId,
      runId: "run-3",
      messages: [{ id: "m-3", role: "user", content: "again" }],
    });
    expect(other.status).toBe(409);
    await post(`/api/threads/${slow.threadId}/cancel`);
    await waitForState(slow.threadId, ["cancelled"]);
    // a finished thread is a conversation (ADR 0020): a message starts its next job
    const late = await postRun(base, "reviewer", {
      threadId: slow.threadId,
      runId: "run-4",
      messages: [{ id: "m-4", role: "user", content: "more" }],
    });
    expect(late.status).toBe(200);
    await late.text();
    await waitForState(slow.threadId, ["done"]);
    const exported = (await (
      await fetch(`${base}/api/threads/${slow.threadId}/export`)
    ).json()) as {
      job: { number: number };
      events: { kind: string; data: { job?: number } }[];
    };
    expect(exported.job.number).toBe(2);
    expect(exported.events.filter((e) => e.kind === "job_started").map((e) => e.data.job)).toEqual([
      2,
    ]);
  });

  it("answers 409 to an action on a card of a finished request", async () => {
    const { threadId } = await startThread("ui pick one", "reviewer");
    await waitForState(threadId, ["blocked"]);
    const action = (runId: string) =>
      postRun(base, "reviewer", {
        threadId,
        runId,
        messages: [],
        forwardedProps: {
          a2uiAction: {
            userAction: { name: "go", surfaceId: "s1", sourceComponentId: "go", context: {} },
          },
        },
      });
    const first = await action("run-2");
    expect(first.status).toBe(200);
    await first.text();
    await waitForState(threadId, ["done"]);
    const late = await action("run-3");
    expect(late.status).toBe(409);
    await expectDocumented("/agui/agents/{agentId}", "post", late);
  });

  it("answers 404 problems for unknown threads", async () => {
    const id = "00000000-0000-4000-8000-00000000ffff";
    const cases: [string, string, string][] = [
      ["/api/threads/{threadId}", "get", `/api/threads/${id}`],
      ["/api/threads/{threadId}/export", "get", `/api/threads/${id}/export`],
      ["/agui/threads/{threadId}/connect", "get", `/agui/threads/${id}/connect`],
    ];
    for (const [tpl, method, p] of cases) {
      const res = await fetch(base + p);
      expect(res.status).toBe(404);
      await expectDocumented(tpl, method, res);
    }
    const res = await post(`/api/threads/${id}/cancel`);
    expect(res.status).toBe(404);
    await expectDocumented("/api/threads/{threadId}/cancel", "post", res);
    const renamed = await patch(`/api/threads/${id}`, { title: "x" });
    expect(renamed.status).toBe(404);
    await expectDocumented("/api/threads/{threadId}", "patch", renamed);
  });

  it("rename (patchThread): the new title, a thread_titled event of the person, a snapshot on the stream", async () => {
    const { threadId } = await startThread("echo");
    await waitForState(threadId, ["done"]);
    const res = await patch(`/api/threads/${threadId}`, { title: "  A better name  " });
    expect(res.status).toBe(200);
    const thread = (await expectDocumented("/api/threads/{threadId}", "patch", res)) as Thread;
    expect(thread).toMatchObject({ id: threadId, title: "A better name", state: "done" });
    const got = await fetch(`${base}/api/threads/${threadId}`);
    expect(((await got.json()) as Thread).title).toBe("A better name");
    const listed = (await (await fetch(`${base}/api/threads`)).json()) as Thread[];
    expect(listed.find((t) => t.id === threadId)?.title).toBe("A better name");

    const doc = (await (await fetch(`${base}/api/threads/${threadId}/export`)).json()) as {
      events: { kind: string; actor: unknown; data: unknown }[];
    };
    const last = doc.events.at(-1);
    expect(last).toMatchObject({
      kind: "thread_titled",
      actor: { type: "user" },
      data: { title: "A better name", source: "user" },
    });
    // the same title again writes nothing
    const again = await patch(`/api/threads/${threadId}`, { title: "A better name" });
    expect(again.status).toBe(200);
    const after = (await (await fetch(`${base}/api/threads/${threadId}/export`)).json()) as {
      events: unknown[];
    };
    expect(after.events).toHaveLength(doc.events.length);

    // a viewer that connects reads the new title, and the rename's own run holds only a snapshot
    const list = await validated(
      await frames(await connect(base, threadId, { mode: "run" })),
      "replay",
    );
    const snapshots = list.filter((f) => f.event.type === "STATE_SNAPSHOT");
    expect(
      snapshots.every(
        (f) => (f.event.snapshot as { thread: { title: string } }).thread.title === "A better name",
      ),
    ).toBe(true);
    expect(types(list).slice(-3)).toEqual(["RUN_STARTED", "STATE_SNAPSHOT", "RUN_FINISHED"]);
  });

  it("rename while the run is open: a state snapshot on the open stream, no run of its own", async () => {
    const { threadId } = await startThread("slow task", "reviewer");
    await waitForState(threadId, ["working"]);
    const stream = await connect(base, threadId);
    expect(stream.status).toBe(200);
    const res = await patch(`/api/threads/${threadId}`, { title: "Mid-run" });
    expect(res.status).toBe(200);
    const seen = await frames(
      stream,
      (f) =>
        f.event.type === "STATE_SNAPSHOT" &&
        (f.event.snapshot as { thread: { title: string } }).thread.title === "Mid-run",
    );
    const last = seen.at(-1);
    expect(last?.id).toBeDefined();
    expect(seen.filter((f) => f.event.type === "RUN_STARTED")).toHaveLength(1);
  });

  it("rename refuses what cannot be a title with a documented 400 and writes nothing", async () => {
    const { threadId } = await startThread("echo");
    await waitForState(threadId, ["done"]);
    const before = (await (await fetch(`${base}/api/threads/${threadId}`)).json()) as Thread;
    const bad: unknown[] = [
      { title: "" },
      { title: "   " },
      { title: "two\nlines" },
      { title: "x".repeat(201) },
      { title: 3 },
      { title: null },
      {},
      { title: "x", state: "done" },
      ["title"],
    ];
    for (const body of bad) {
      const res = await patch(`/api/threads/${threadId}`, body);
      expect(res.status, JSON.stringify(body)).toBe(400);
      await expectDocumented("/api/threads/{threadId}", "patch", res);
    }
    const notJson = await fetch(`${base}/api/threads/${threadId}`, {
      method: "PATCH",
      headers: { "Content-Type": "application/json" },
      body: "not json",
    });
    expect(notJson.status).toBe(400);
    await expectDocumented("/api/threads/{threadId}", "patch", notJson);
    const after = (await (await fetch(`${base}/api/threads/${threadId}`)).json()) as Thread;
    expect(after).toEqual(before);
    const fits = await patch(`/api/threads/${threadId}`, { title: "x".repeat(200) });
    expect(fits.status).toBe(200);
  });

  it("describe (patchThread): the model's description arrives with the thread, a person's replaces it and clears it", async () => {
    const { threadId } = await startThread("describe talk to me", "reviewer");
    await waitForState(threadId, ["done"]);
    const get = async () =>
      (await (await fetch(`${base}/api/threads/${threadId}`)).json()) as Thread;
    // the model describes the thread a step after its job ends
    for (let i = 0; i < 200 && !(await get()).description; i++) {
      await new Promise((r) => setTimeout(r, 5));
    }
    expect((await get()).description).toBe("The person wants a plan for a test.");
    const listed = (await (await fetch(`${base}/api/threads`)).json()) as Thread[];
    expect(listed.find((t) => t.id === threadId)?.description).toBe(
      "The person wants a plan for a test.",
    );

    // a person writes one: trimmed, in the thread, the list and the export, as an event of the person
    const res = await patch(`/api/threads/${threadId}`, { description: "  Plan the test.  " });
    expect(res.status).toBe(200);
    const written = (await expectDocumented("/api/threads/{threadId}", "patch", res)) as Thread;
    expect(written.description).toBe("Plan the test.");
    const doc = (await (await fetch(`${base}/api/threads/${threadId}/export`)).json()) as {
      thread: Thread;
      events: { kind: string; actor: unknown; data: unknown }[];
    };
    expect(doc.thread.description).toBe("Plan the test.");
    expect(doc.events.at(-1)).toMatchObject({
      kind: "thread_described",
      actor: { type: "user" },
      data: { description: "Plan the test.", source: "user" },
    });
    // the same one again writes nothing
    expect(
      (await patch(`/api/threads/${threadId}`, { description: "Plan the test." })).status,
    ).toBe(200);
    const again = (await (await fetch(`${base}/api/threads/${threadId}/export`)).json()) as {
      events: unknown[];
    };
    expect(again.events).toHaveLength(doc.events.length);

    // both members in one request; the title and the description are each written
    const both = await patch(`/api/threads/${threadId}`, { title: "Test plan", description: "x" });
    expect(both.status).toBe(200);
    expect(await both.json()).toMatchObject({ title: "Test plan", description: "x" });

    // a person's description is final: an empty one clears it and stays cleared
    const cleared = await patch(`/api/threads/${threadId}`, { description: "" });
    expect(cleared.status).toBe(200);
    expect(((await cleared.json()) as Thread).description).toBeUndefined();
    expect((await get()).description).toBeUndefined();
    // a viewer reads no description in the replay's last snapshot
    const list = await validated(
      await frames(await connect(base, threadId, { mode: "run" })),
      "replay",
    );
    const last = list.filter((f) => f.event.type === "STATE_SNAPSHOT").at(-1);
    const snapshot = last?.event.snapshot as { thread: Record<string, unknown> } | undefined;
    expect(snapshot?.thread).toBeDefined();
    expect(snapshot?.thread).not.toHaveProperty("description");
  });

  it("describe refuses what cannot be a description with a documented 400 and writes nothing", async () => {
    const { threadId } = await startThread("echo");
    await waitForState(threadId, ["done"]);
    const before = (await (await fetch(`${base}/api/threads/${threadId}`)).json()) as Thread;
    const bad: unknown[] = [
      { description: "two\nlines" },
      { description: "x".repeat(501) },
      { description: 3 },
      { description: null },
      { description: "ok", title: "" }, // both are checked before either is written
      { description: "ok", state: "done" },
    ];
    for (const body of bad) {
      const res = await patch(`/api/threads/${threadId}`, body);
      expect(res.status, JSON.stringify(body)).toBe(400);
      await expectDocumented("/api/threads/{threadId}", "patch", res);
    }
    expect((await (await fetch(`${base}/api/threads/${threadId}`)).json()) as Thread).toEqual(
      before,
    );
    expect((await patch(`/api/threads/${threadId}`, { description: "x".repeat(500) })).status).toBe(
      200,
    );
  });

  it("a description a person wrote is not replaced by the model's, which comes after it", async () => {
    const slow = createMockServer({ stepMs: 40, keepaliveMs: 1000 });
    await new Promise<void>((r) => slow.listen(0, "127.0.0.1", r));
    // nosemgrep: opt.opengrep-rules.typescript.react.security.react-insecure-request -- loopback test server, never leaves the runner
    const url = `http://127.0.0.1:${(slow.address() as AddressInfo).port}`;
    try {
      const id = newId();
      const run = await fetch(`${url}/agui/agents/reviewer`, {
        method: "POST",
        headers: { "Content-Type": "application/json", Accept: "text/event-stream" },
        body: JSON.stringify({
          threadId: id,
          runId: "run-1",
          messages: [{ id: "m-1", role: "user", content: "describe talk to me" }],
        }),
      });
      await run.body?.cancel();
      const mine = await fetch(`${url}/api/threads/${id}`, {
        method: "PATCH",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ description: "Mine." }),
      });
      expect(mine.status).toBe(200);
      await new Promise((r) => setTimeout(r, 600));
      const thread = (await (await fetch(`${url}/api/threads/${id}`)).json()) as Thread;
      expect(thread.state).toBe("done");
      expect(thread.description).toBe("Mine.");
    } finally {
      slow.closeAllConnections();
      await new Promise<void>((r) => slow.close(() => r()));
    }
  });

  it("config (getConfig): the ui section with showDescriptions, switched per session by a test hook", async () => {
    const res = await fetch(`${base}/api/config`);
    expect(res.status).toBe(200);
    expect(await expectDocumented("/api/config", "get", res)).toEqual({
      ui: { showDescriptions: true },
    });
    const cookie = { Cookie: "mock-registry=config-test" };
    expect((await post("/__mock/config?showDescriptions=false&session=config-test")).status).toBe(
      204,
    );
    const off = await fetch(`${base}/api/config`, { headers: cookie });
    expect(await off.json()).toEqual({ ui: { showDescriptions: false } });
    // another session keeps its own
    expect(await (await fetch(`${base}/api/config`)).json()).toEqual({
      ui: { showDescriptions: true },
    });
  });

  it("export: the thread as a ThreadExport attachment, with its whole log", async () => {
    const { threadId } = await startThread("echo");
    await waitForState(threadId, ["done"]);
    const res = await fetch(`${base}/api/threads/${threadId}/export`);
    expect(res.status).toBe(200);
    expect(res.headers.get("content-disposition")).toBe(
      `attachment; filename="thread-${threadId}.json"`,
    );
    const doc = (await expectDocumented("/api/threads/{threadId}/export", "get", res)) as {
      thread: Thread;
      events: { seq: number }[];
    };
    expect(doc.thread.id).toBe(threadId);
    expect(doc.events.map((e) => e.seq)).toEqual(doc.events.map((_, i) => i + 1));
    expect(doc.events.length).toBe(doc.thread.lastSeq);
  });

  it("artifact: a file the thread holds, inline or as an attachment, and the 404s (ADR 0032)", async () => {
    const { threadId } = await startThread("file make a chart", "reviewer");
    await waitForState(threadId, ["done"]);
    const doc = (await (await fetch(`${base}/api/threads/${threadId}/export`)).json()) as {
      events: { kind: string; data: { file?: { sha256: string; size: number } } }[];
    };
    const file = doc.events.find((e) => e.kind === "artifact")?.data.file;
    expect(file).toBeDefined();
    const sha = file?.sha256 ?? "";
    const path = `/api/threads/${threadId}/artifacts/${sha}`;

    const inline = await fetch(base + path);
    expect(inline.status).toBe(200);
    expect(inline.headers.get("content-type")).toBe("image/png");
    expect(inline.headers.get("content-disposition")).toBe('inline; filename="chart.png"');
    expect(inline.headers.get("x-content-type-options")).toBe("nosniff");
    expect(inline.headers.get("content-security-policy")).toBe(
      "default-src 'none'; img-src 'self' data:; style-src 'unsafe-inline'; sandbox",
    );
    expect(inline.headers.get("cache-control")).toBe("private, max-age=31536000, immutable");
    expect((await inline.arrayBuffer()).byteLength).toBe(file?.size);

    const download = await fetch(`${base}${path}?download=1`);
    expect(download.headers.get("content-disposition")).toBe('attachment; filename="chart.png"');

    for (const missing of [
      `/api/threads/${threadId}/artifacts/${"0".repeat(64)}`,
      `/api/threads/${newId()}/artifacts/${sha}`,
    ]) {
      const res = await fetch(base + missing);
      expect(res.status).toBe(404);
      await expectDocumented("/api/threads/{threadId}/artifacts/{sha256}", "get", res);
    }
    const bad = await fetch(`${base}${path}?download=2`);
    expect(bad.status).toBe(400);
    await expectDocumented("/api/threads/{threadId}/artifacts/{sha256}", "get", bad);
  });

  it("artifact: the three kinds of kept file of the `files` scenario, each served as its preview says", async () => {
    const { threadId } = await startThread("files make some", "reviewer");
    await waitForState(threadId, ["done"]);
    const doc = (await (await fetch(`${base}/api/threads/${threadId}/export`)).json()) as {
      events: { kind: string; data: { file?: { sha256: string; filename: string } } }[];
    };
    const files = doc.events.flatMap((e) =>
      e.kind === "artifact" && e.data.file ? [e.data.file] : [],
    );
    expect(files.map((f) => f.filename)).toEqual(["results.png", "notes.txt", "export.zip"]);
    const served = await Promise.all(
      files.map((f) => fetch(`${base}/api/threads/${threadId}/artifacts/${f.sha256}`)),
    );
    expect(served.map((r) => r.headers.get("content-type"))).toEqual([
      "image/png",
      "text/plain; charset=utf-8",
      "application/zip",
    ]);
    // a preview type is inline, an archive is an attachment even without ?download=1
    expect(served.map((r) => r.headers.get("content-disposition")?.split(";")[0])).toEqual([
      "inline",
      "inline",
      "attachment",
    ]);
    expect(await served[1]?.text()).toContain("Results of the run");
  });

  it("connect route rejects a bad cursor and a bad mode with 400", async () => {
    const { threadId } = await startThread("echo");
    for (const res of [
      await fetch(`${base}/agui/threads/${threadId}/connect`, {
        headers: { "Last-Event-ID": "x" },
      }),
      await fetch(`${base}/agui/threads/${threadId}/connect?mode=forever`),
    ]) {
      expect(res.status).toBe(400);
      await expectDocumented("/agui/threads/{threadId}/connect", "get", res);
    }
  });

  it("capabilities: an AgentCapabilities, with the release channels only for coder", async () => {
    for (const [agent, releases] of [
      ["adam", true],
      ["reviewer", false],
    ] as const) {
      const res = await fetch(`${base}/agui/agents/${agent}/capabilities`);
      const doc = (await expectDocumented("/agui/agents/{agentId}/capabilities", "get", res)) as {
        custom?: Record<string, unknown>;
      };
      expect(doc.custom !== undefined).toBe(releases);
    }
    const missing = await fetch(`${base}/agui/agents/nope/capabilities`);
    expect(missing.status).toBe(404);
    await expectDocumented("/agui/agents/{agentId}/capabilities", "get", missing);
  });

  it("cancel: 202, then the cancelled outcome on the stream", async () => {
    const { threadId } = await startThread("slow task", "reviewer");
    await waitForState(threadId, ["working"]);
    const res = await post(`/api/threads/${threadId}/cancel`);
    expect(res.status).toBe(202);
    await expectDocumented("/api/threads/{threadId}/cancel", "post", res);
    await waitForState(threadId, ["cancelled"]);
    const list = await frames(await connect(base, threadId, { mode: "run" }));
    expect(list.at(-1)?.event).toMatchObject({
      type: "RUN_FINISHED",
      outcome: { type: "cancelled" },
    });
  });

  it("agent failure: a failed status with the agent's detail, RUN_ERROR agent_failed, no error activity", async () => {
    const { threadId, body } = await startThread("fail please", "reviewer");
    await waitForState(threadId, ["failed"]);
    expect(body.at(-1)?.event).toMatchObject({
      type: "RUN_ERROR",
      code: "agent_failed",
      message: "scripted failure",
    });
    expect(JSON.stringify(body)).not.toContain("vymalo.error");
  });

  it("partial (mock only): one message, said once", async () => {
    const { threadId } = await startThread("partial please");
    await waitForState(threadId, ["done"]);
    const list = await frames(await connect(base, threadId, { mode: "run" }));
    const say = list.filter(
      (f) => f.event.type === "TEXT_MESSAGE_CONTENT" && !f.event.subagentRunId === false,
    );
    expect(say.map((f) => f.event.delta).join("")).toContain(
      "I'll start with the failing test, then make",
    );
    expect(
      list.filter((f) => f.event.type === "TEXT_MESSAGE_END" && f.event.subagentRunId),
    ).toHaveLength(1);
  });

  it("unreachable (mock only): an error activity, then RUN_ERROR delivery_failed and blocked", async () => {
    const { threadId, body } = await startThread("unreachable agent");
    await waitForState(threadId, ["blocked"]);
    expect(body.at(-1)?.event).toMatchObject({ type: "RUN_ERROR", code: "delivery_failed" });
    expect(JSON.stringify(body)).toContain('"activityType":"vymalo.error"');
  });

  it("verify-red-once (the gate): the run stays open across the attempts, and the thread says where its job stands", async () => {
    const { threadId, body } = await startThread("verify-red-once fix the login", "reviewer");
    // one run, from RUN_STARTED to RUN_FINISHED success; the agent finishing twice does not end it
    expect(types(body).filter((t) => t === "RUN_STARTED")).toHaveLength(1);
    expect(types(body).filter((t) => t === "SUBAGENT_FINISHED")).toHaveLength(2);
    expect(body.at(-1)?.event).toMatchObject({
      type: "RUN_FINISHED",
      outcome: { type: "success" },
    });
    const thread = await waitForState(threadId, ["done"]);
    expect(thread.job).toEqual({
      attempt: 2,
      maxAttempts: 3,
      gate: ["agent_checks"],
      sha: "0000000000000000000000000000000000000002",
    });
    // the list carries the job too, and a thread without a gate has none
    const listed = (await expectDocumented(
      "/api/threads",
      "get",
      await fetch(`${base}/api/threads?limit=100`),
    )) as Thread[];
    expect(listed.find((t) => t.id === threadId)?.job?.attempt).toBe(2);
    const plain = await startThread("echo hi", "reviewer");
    expect((await waitForState(plain.threadId, ["done"])).job).toBeUndefined();
  });

  it("verify-red: RUN_ERROR checks_failed after the last attempt, thread failed on attempt 3 of 3", async () => {
    const { threadId, body } = await startThread("verify-red fix the login", "reviewer");
    expect(body.at(-1)?.event).toMatchObject({
      type: "RUN_ERROR",
      code: "checks_failed",
      metadata: { "vymalo.problem": { title: "Checks failed" } },
    });
    const thread = await waitForState(threadId, ["failed"]);
    expect(thread.job).toMatchObject({ attempt: 3, maxAttempts: 3 });
  });

  it("verify-reviewed (the verifier agent of the gate): the verifier is a subagent of its own, with its verdict as the result", async () => {
    const { threadId, body } = await startThread("verify-reviewed fix the login", "reviewer");
    const events = body.map((f) => f.event);
    const started = events.filter(
      (e) => e.type === "SUBAGENT_STARTED" && String(e.subagentRunId).startsWith("sub-verify-"),
    );
    // one per verification, named after the verifier agent, the id the real projection gives it
    expect(started.map((e) => [e.subagentRunId, e.name])).toEqual([
      ["sub-verify-1", "verifier"],
      ["sub-verify-2", "verifier"],
    ]);
    const finished = events.filter(
      (e) => e.type === "SUBAGENT_FINISHED" && String(e.subagentRunId).startsWith("sub-verify-"),
    );
    expect(finished.map((e) => [e.subagentRunId, e.result])).toEqual([
      ["sub-verify-1", { passed: false }],
      ["sub-verify-2", { passed: true }],
    ]);
    // it starts on its pending card and ends on its verdict, never before or after
    const at = (type: string, run: string) =>
      events.findIndex((e) => e.type === type && e.subagentRunId === run);
    const card = (status: string, attempt: number) =>
      events.findIndex(
        (e) =>
          e.activityType === "vymalo.check" &&
          (e.content as { status?: string; attempt?: number }).status === status &&
          (e.content as { attempt?: number }).attempt === attempt,
      );
    expect(at("SUBAGENT_STARTED", "sub-verify-1")).toBe(card("pending", 1) - 1);
    expect(at("SUBAGENT_FINISHED", "sub-verify-1")).toBe(card("failed", 1) + 1);
    expect(at("SUBAGENT_FINISHED", "sub-verify-2")).toBe(card("passed", 2) + 1);
    const thread = await waitForState(threadId, ["done"]);
    expect(thread.job).toMatchObject({ attempt: 2, maxAttempts: 3, gate: ["verifier"] });
  });

  it("verify-reviewed-red: three verdicts of findings, RUN_ERROR checks_failed, every verifier subagent closed", async () => {
    const { threadId, body } = await startThread("verify-reviewed-red fix the login", "reviewer");
    expect(body.at(-1)?.event).toMatchObject({ type: "RUN_ERROR", code: "checks_failed" });
    const results = body
      .map((f) => f.event)
      .filter(
        (e) => e.type === "SUBAGENT_FINISHED" && String(e.subagentRunId).startsWith("sub-verify-"),
      )
      .map((e) => e.result);
    expect(results).toEqual([{ passed: false }, { passed: false }, { passed: false }]);
    expect((await waitForState(threadId, ["failed"])).job).toMatchObject({ attempt: 3 });
  });

  it("verify-reviewed-wait (mock only): a client that joins is told about the verifier, and Cancel ends it as canceled", async () => {
    const { threadId } = await startThread("verify-reviewed-wait ship it", "reviewer");
    for (let i = 0; i < 200; i++) {
      const t = (await (await fetch(`${base}/api/threads/${threadId}`)).json()) as Thread;
      if (t.lastSeq >= 6) break; // the verifier's pending card is the 6th event: the script now waits
      await new Promise((r) => setTimeout(r, 10));
    }
    const ac = new AbortController();
    const live = frames(await connect(base, threadId, { lastEventId: 6, signal: ac.signal }), (f) =>
      isTerminal(f),
    );
    expect((await post(`/api/threads/${threadId}/cancel`)).status).toBe(202);
    const list = await live;
    ac.abort();
    expect(list.slice(0, 3).map((f) => f.event.type)).toEqual([
      "RUN_STARTED",
      "SUBAGENT_STARTED",
      "STATE_SNAPSHOT",
    ]);
    expect(list[1]?.event).toMatchObject({ subagentRunId: "sub-verify-1", name: "verifier" });
    expect(list.slice(0, 3).every((f) => f.id === undefined)).toBe(true);
    const finished = list.find((f) => f.event.type === "SUBAGENT_FINISHED");
    expect(finished?.event).toMatchObject({
      subagentRunId: "sub-verify-1",
      result: { status: "canceled" },
    });
    expect(list.at(-1)?.event).toMatchObject({
      type: "RUN_FINISHED",
      outcome: { type: "cancelled" },
    });
    await waitForState(threadId, ["cancelled"]);
  });

  it("verify-ci (the orchestrator's CI story): a red report, a rework, a green report; a vymalo.ci card for each report", async () => {
    const { threadId, body } = await startThread("verify-ci fix the login", "reviewer");
    expect(body.at(-1)?.event).toMatchObject({
      type: "RUN_FINISHED",
      outcome: { type: "success" },
    });
    const thread = await waitForState(threadId, ["done"]);
    expect(thread.job).toMatchObject({ attempt: 2, gate: ["ci"] });
    const cards = body.filter((f) => f.event.activityType === "vymalo.ci");
    expect(cards.map((f) => f.event.content)).toMatchObject([
      {
        name: "ci/build",
        conclusion: "failure",
        passed: false,
        shortSha: "0000000",
        provider: "generic",
        repository: "github.com/acme/demo",
        url: "https://ci.example.com/runs/1",
      },
      { name: "ci/build", conclusion: "success", passed: true, summary: "3 tests passed" },
    ]);
    // the orchestrator's own card (no subagent), one per report
    expect(cards.every((f) => f.event.subagentRunId === undefined)).toBe(true);
    expect(new Set(cards.map((f) => f.event.messageId)).size).toBe(2);
    const order = body
      .filter((f) =>
        ["vymalo.check", "vymalo.ci", "vymalo.rework"].includes(String(f.event.activityType)),
      )
      .map(
        (f) =>
          `${String(f.event.activityType)}:${String((f.event.content as { status?: string }).status ?? "")}`,
      );
    expect(order).toEqual([
      "vymalo.check:pending",
      "vymalo.ci:",
      "vymalo.check:failed",
      "vymalo.rework:",
      "vymalo.check:pending",
      "vymalo.ci:",
      "vymalo.check:passed",
    ]);
  });

  it("verify-ci-stale (mock only): every report has a card, a late one for an older push included", async () => {
    const { threadId, body } = await startThread("verify-ci-stale ship it", "reviewer");
    await waitForState(threadId, ["done"]);
    const cards = body.filter((f) => f.event.activityType === "vymalo.ci");
    expect(
      cards.map((f) => (f.event.content as { sha: string; conclusion: string }).conclusion),
    ).toEqual(["failure", "success"]);
    expect(new Set(cards.map((f) => f.event.messageId)).size).toBe(2);
  });

  it("verify-wait (mock only): the thread stays verifying with a pending CI check; Cancel ends it", async () => {
    const { threadId } = await startThread("verify-wait ship it", "reviewer");
    await waitForState(threadId, ["verifying"]);
    for (let i = 0; i < 200; i++) {
      const t = (await (await fetch(`${base}/api/threads/${threadId}`)).json()) as Thread;
      if (t.lastSeq >= 7) break; // the pending CI check is the 7th event: the script now waits
      await new Promise((r) => setTimeout(r, 10));
    }
    // a message while a run is open is refused, verifying included
    const busy = await postRun(base, "reviewer", {
      threadId,
      runId: "run-2",
      messages: [{ id: "m-2", role: "user", content: "hello?" }],
    });
    expect(busy.status).toBe(409);
    await expectDocumented("/agui/agents/{agentId}", "post", busy);
    expect((await post(`/api/threads/${threadId}/cancel`)).status).toBe(202);
    const cancelled = await waitForState(threadId, ["cancelled"]);
    expect(cancelled.job).toMatchObject({ attempt: 1, gate: ["ci", "agent_checks"] });
    const list = await frames(await connect(base, threadId, { mode: "run" }));
    const cards = list.filter((f) => f.event.activityType === "vymalo.check");
    expect(cards.map((f) => f.event.messageId)).toEqual(["check-1-1-agent_checks", "check-1-1-ci"]);
    expect(list.at(-1)?.event).toMatchObject({
      type: "RUN_FINISHED",
      outcome: { type: "cancelled" },
    });
  });

  it("ui (mock only, the orchestrator's a2ui story): a surface, then an action that continues the scenario", async () => {
    const { threadId, body } = await startThread("ui pick one", "reviewer");
    expect(body.at(-1)?.event).toMatchObject({
      type: "RUN_FINISHED",
      outcome: { type: "interrupt" },
    });
    const surfaces = body.filter((f) => f.event.activityType === "a2ui-surface");
    // two snapshots of ONE surface (one message id); the second holds both payloads
    expect(new Set(surfaces.map((f) => f.event.messageId)).size).toBe(1);
    expect(surfaces).toHaveLength(2);
    expect(
      surfaces.map(
        (f) => (f.event.content as { a2ui_operations: unknown[] }).a2ui_operations.length,
      ),
    ).toEqual([1, 2]);
    await waitForState(threadId, ["blocked"]);

    const action = (over: Record<string, unknown> = {}, id = newId()) => ({
      threadId,
      runId: id,
      messages: [],
      forwardedProps: {
        a2uiAction: {
          userAction: {
            name: "go",
            surfaceId: "s1",
            sourceComponentId: "go",
            context: { choice: "a" },
            ...over,
          },
        },
      },
    });
    // refused before anything is written: shape, size, a surface the thread does not have
    const refusals: [number, Record<string, unknown>][] = [
      [422, { name: 3 }],
      [422, { sourceComponentId: undefined }],
      [422, { surfaceId: "other" }],
      [422, { context: [] }],
      [413, { name: "x".repeat(257) }],
      [413, { context: { big: "x".repeat(17 * 1024) } }],
    ];
    for (const [status, over] of refusals) {
      const res = await postRun(base, "reviewer", action(over) as Parameters<typeof postRun>[2]);
      expect(res.status, JSON.stringify(over).slice(0, 80)).toBe(status);
      await expectDocumented("/agui/agents/{agentId}", "post", res);
    }
    // an action together with a message is refused; the thread still waits
    const together = {
      ...action(),
      messages: [{ id: "m-x", role: "user" as const, content: "hi" }],
    };
    expect((await postRun(base, "reviewer", together)).status).toBe(422);
    expect((await fetch(`${base}/api/threads/${threadId}`).then((r) => r.json())).state).toBe(
      "blocked",
    );

    const ok = await postRun(base, "reviewer", action() as Parameters<typeof postRun>[2]);
    expect(ok.status).toBe(200);
    const run = await validated(await frames(ok), "action run frames");
    expect(run.at(-1)?.event).toMatchObject({ type: "RUN_FINISHED", outcome: { type: "success" } });
    expect(JSON.stringify(run)).toContain("answered: ui-action go");
    await waitForState(threadId, ["done"]);
    // a finished thread takes no action
    const late = await postRun(base, "reviewer", action() as Parameters<typeof postRun>[2]);
    expect(late.status).toBe(409);
    await expectDocumented("/agui/agents/{agentId}", "post", late);
    // and neither does a thread that does not exist
    const none = await postRun(base, "reviewer", {
      ...action(),
      threadId: newId(),
    } as Parameters<typeof postRun>[2]);
    expect(none.status).toBe(422);
  });

  it("an action is refused while a run is open", async () => {
    const { threadId } = await startThread("ui pick one", "reviewer");
    await waitForState(threadId, ["blocked"]);
    const first = await postRun(base, "reviewer", {
      threadId,
      runId: newId(),
      messages: [],
      forwardedProps: {
        a2uiAction: {
          userAction: { name: "go", surfaceId: "s1", sourceComponentId: "go", context: {} },
        },
      },
    });
    expect(first.status).toBe(200);
    // the run of the first action is open now (the response is still streaming): a second is 409
    const second = await postRun(base, "reviewer", {
      threadId,
      runId: newId(),
      messages: [],
      forwardedProps: {
        a2uiAction: {
          userAction: { name: "go", surfaceId: "s1", sourceComponentId: "go", context: {} },
        },
      },
    });
    expect(second.status).toBe(409);
    // and so is one that says how it is delivered: only a message is sent while the agent works
    // (ADR 0036; the orchestrator's `finish` answers RunInProgress)
    for (const send of ["steer", "interrupt"]) {
      const sent = await postRun(base, "reviewer", {
        threadId,
        runId: newId(),
        messages: [],
        forwardedProps: {
          "vymalo.send": send,
          a2uiAction: {
            userAction: { name: "go", surfaceId: "s1", sourceComponentId: "go", context: {} },
          },
        },
      });
      expect(sent.status, send).toBe(409);
    }
    await first.text();
  });

  it("lists threads newest first, with limit and before", async () => {
    const res = await fetch(`${base}/api/threads?limit=100`);
    const all = (await expectDocumented("/api/threads", "get", res)) as Thread[];
    expect(all.length).toBeGreaterThanOrEqual(3);
    const times = all.map((t) => t.createdAt);
    expect([...times].sort().reverse()).toEqual(times);
    const r2 = await fetch(`${base}/api/threads?limit=1`);
    const one = (await expectDocumented("/api/threads", "get", r2)) as Thread[];
    expect(one.map((t) => t.id)).toEqual([all[0]?.id]);
    const r3 = await fetch(`${base}/api/threads?limit=1&before=${all[0]?.id}`);
    const next = (await expectDocumented("/api/threads", "get", r3)) as Thread[];
    expect(next.map((t) => t.id)).toEqual([all[1]?.id]);
  });
});

describe("the UI catalog (ADR 0023), as the mock records it", () => {
  const ref = (version = OWN_CATALOG.version, digest = OWN_CATALOG.digest) => ({
    catalogId: OWN_CATALOG.catalogId,
    version,
    digest,
  });
  /** A catalog that is not the build's: one more component, a version and a digest of its own. */
  const catalogV = async (version: number) => {
    const catalog = {
      ...OWN_CATALOG.catalog,
      components: {
        ...OWN_CATALOG.catalog.components,
        [`Extra${version}`]: {
          type: "object",
          properties: { component: { const: `Extra${version}` } },
        },
      },
    };
    return { ...OWN_CATALOG, version, catalog, digest: await catalogDigest(catalog) };
  };
  /** The `uiCatalog` of each snapshot of a run. */
  const snapshots = (list: Frame[]) =>
    list
      .filter((f) => f.event.type === "STATE_SNAPSHOT")
      .map((f) => (f.event.snapshot as { thread: { uiCatalog?: unknown } }).thread.uiCatalog);

  it("records the catalog a run carries, and says it in every snapshot of the thread", async () => {
    const { body } = await startThread("Implement the thing", "adam", {
      forwardedProps: { [UI_CATALOG_PROP]: OWN_CATALOG },
    });
    const says = snapshots(body);
    expect(says.length).toBeGreaterThan(0);
    for (const s of says) expect(s).toEqual(ref());
  });

  it("a thread nobody told about the catalog has none in its snapshots", async () => {
    const { body } = await startThread("Implement the thing");
    expect(snapshots(body).every((s) => s === undefined)).toBe(true);
  });

  it("refuses a malformed catalog before anything is written", async () => {
    const bad: [number, string, unknown][] = [
      [400, "not an object", "text"],
      [
        400,
        "a digest that is not the catalog's",
        { ...OWN_CATALOG, digest: `sha256:${"0".repeat(64)}` },
      ],
      [400, "a malformed digest", { ...OWN_CATALOG, digest: "sha256:ABC" }],
      [400, "a version of 0", { ...OWN_CATALOG, version: 0 }],
      [400, "a version over a million", { ...OWN_CATALOG, version: 1_000_001 }],
      [400, "a catalogId that is not https", { ...OWN_CATALOG, catalogId: "http://x.test/c" }],
      [
        400,
        "a catalog of another catalogId",
        { ...OWN_CATALOG, catalogId: "https://x.test/other" },
      ],
      [
        400,
        "a component name that is not a name",
        {
          ...OWN_CATALOG,
          catalog: { ...OWN_CATALOG.catalog, components: { lower: {} } },
        },
      ],
      [
        413,
        "a catalog over 64 KiB",
        {
          ...OWN_CATALOG,
          catalog: {
            ...OWN_CATALOG.catalog,
            components: { Big: { description: "x".repeat(65 * 1024) } },
          },
        },
      ],
    ];
    for (const [status, label, value] of bad) {
      const threadId = newId();
      const res = await postRun(base, "adam", {
        threadId,
        runId: "run-1",
        messages: [{ id: "m-1", role: "user", content: "Implement the thing" }],
        forwardedProps: { [UI_CATALOG_PROP]: value },
      });
      expect(res.status, label).toBe(status);
      await expectDocumented("/agui/agents/{agentId}", "post", res);
      // nothing was created
      expect((await fetch(`${base}/api/threads/${threadId}`)).status, label).toBe(404);
    }
  });

  it("a newer catalog in a later run becomes the thread's; an older one never does", async () => {
    const v1 = await catalogV(1);
    const v2 = await catalogV(2);
    const { threadId } = await startThread("ask a question", "adam", {
      forwardedProps: { [UI_CATALOG_PROP]: v1 },
    });
    await waitForState(threadId, ["blocked"]);
    // the answer carries v2: the snapshots of that run say 2
    const answer = await postRun(base, "adam", {
      threadId,
      runId: newId(),
      messages: [],
      resume: [{ interruptId: "int-3", status: "resolved", payload: { text: "main" } }],
      forwardedProps: { [UI_CATALOG_PROP]: v2 },
    });
    expect(answer.status).toBe(200);
    const run = await validated(await frames(answer), "answer");
    for (const s of snapshots(run)) expect(s).toEqual(ref(2, v2.digest));
    await waitForState(threadId, ["done"]);
    // an older UI's catalog on the next job is recorded and not current
    const next = await postRun(base, "adam", {
      threadId,
      runId: newId(),
      messages: [{ id: "m-next", role: "user", content: "Also this" }],
      forwardedProps: { [UI_CATALOG_PROP]: v1 },
    });
    expect(next.status).toBe(200);
    const again = await validated(await frames(next), "next job");
    for (const s of snapshots(again)) expect(s).toEqual(ref(2, v2.digest));
  });

  it("records each catalog in the log, first in its commit and once per digest, with no frame of its own", async () => {
    const v1 = await catalogV(1);
    const v2 = await catalogV(2);
    const { threadId, body } = await startThread("echo hi", "reviewer", {
      forwardedProps: { [UI_CATALOG_PROP]: v1 },
    });
    // the run opens with the message: a catalog has no frame, the snapshots say which one counts
    expect(body[0]?.event.type).toBe("RUN_STARTED");
    await waitForState(threadId, ["done"]);
    const run = async (n: number, props: unknown) => {
      const res = await postRun(base, "reviewer", {
        threadId,
        runId: newId(),
        messages: [{ id: `m-${n}`, role: "user", content: "echo again" }],
        forwardedProps: { [UI_CATALOG_PROP]: props },
      });
      expect(res.status).toBe(200);
      await frames(res);
      await waitForState(threadId, ["done"]);
    };
    // the digest again is not recorded twice; a newer one is; an older one is recorded, never current
    await run(2, v1);
    await run(3, v2);
    const olderCatalog = {
      ...OWN_CATALOG.catalog,
      components: {
        ...OWN_CATALOG.catalog.components,
        Older: { type: "object", properties: { component: { const: "Older" } } },
      },
    };
    const older = { ...v1, catalog: olderCatalog, digest: await catalogDigest(olderCatalog) };
    await run(4, older);
    const res = await fetch(`${base}/api/threads/${threadId}/export`);
    const doc = (await expectDocumented("/api/threads/{threadId}/export", "get", res)) as {
      events: { seq: number; kind: string; data: { version?: number; digest?: string } }[];
    };
    const kinds = doc.events.map((e) => e.kind);
    expect(kinds[0]).toBe("ui_catalog");
    expect(doc.events.filter((e) => e.kind === "ui_catalog").map((e) => e.data.digest)).toEqual([
      v1.digest,
      v2.digest,
      older.digest,
    ]);
    // each catalog comes right before the message that carried it
    for (const [i, e] of doc.events.entries()) {
      if (e.kind === "ui_catalog") expect(kinds[i + 1]).toBe("user_message");
    }
    // a replay shows the catalog as of each point: version 1, then 2 from the third job on, and
    // the older one that came last never counts
    const replay = snapshots(await frames(await connect(base, threadId, { mode: "run" })));
    expect(replay[0]).toEqual(ref(1, v1.digest));
    expect(replay.at(-1)).toEqual(ref(2, v2.digest));
    const versions = replay.map((s) => (s as { version: number }).version);
    expect([...versions].sort((a, b) => a - b)).toEqual(versions);
  });

  it("catalog-newer (mock only): the thread's catalog is version 99, whatever the web sent", async () => {
    const { body } = await startThread("catalog-newer please", "reviewer", {
      forwardedProps: { [UI_CATALOG_PROP]: OWN_CATALOG },
    });
    for (const s of snapshots(body)) {
      expect(s).toMatchObject({ catalogId: OWN_CATALOG.catalogId, version: 99 });
    }
    const surface = body.find((f) => f.event.activityType === "a2ui-surface");
    expect(JSON.stringify(surface?.event.content)).toContain("Gizmo");
  });
});

describe("choices (mock only): a Choices of three questions, and the answer", () => {
  it("sends the surface under the web's catalog, asks, and echoes what the answers chose", async () => {
    const { threadId, body } = await startThread("choices please", "reviewer", {
      forwardedProps: { [UI_CATALOG_PROP]: OWN_CATALOG },
    });
    expect(body.at(-1)?.event).toMatchObject({
      type: "RUN_FINISHED",
      outcome: { type: "interrupt" },
    });
    const surfaces = body.filter((f) => f.event.activityType === "a2ui-surface");
    const content = surfaces.at(-1)?.event.content as {
      a2ui_operations: Record<string, unknown>[];
    };
    const ops = content.a2ui_operations;
    expect(ops[0]).toMatchObject({
      createSurface: { surfaceId: "s1", catalogId: OWN_CATALOG.catalogId },
    });
    expect(JSON.stringify(ops)).toContain('"component":"Choices"');
    await waitForState(threadId, ["blocked"]);

    const answer = await postRun(base, "reviewer", {
      threadId,
      runId: newId(),
      messages: [],
      forwardedProps: {
        a2uiAction: {
          userAction: {
            name: "answer",
            surfaceId: "s1",
            sourceComponentId: "pick",
            context: {
              answers: [
                { id: "db", values: [], other: "Cockroach" },
                { id: "auth", values: ["none"] },
                { id: "deploy", values: ["k8s", "compose"] },
              ],
            },
          },
        },
      },
    });
    expect(answer.status).toBe(200);
    const run = await validated(await frames(answer), "answer run");
    expect(JSON.stringify(run)).toContain(
      "answered: ui-action answer db=other:Cockroach auth=none deploy=k8s,compose",
    );
    // the answer is in the log as the person's action, with its context
    const action = run.find((f) => f.event.activityType === "vymalo.action");
    expect(action?.event.content).toMatchObject({
      name: "answer",
      sourceComponentId: "pick",
      context: { answers: [{ id: "db" }, { id: "auth" }, { id: "deploy" }] },
    });
    await waitForState(threadId, ["done"]);
  });
});

describe("cards-mermaid (mock only): text, three cards and a graph in one answer", () => {
  const surfaceOf = (body: { event: Record<string, unknown> }[]) => {
    const surfaces = body.filter((f) => f.event.activityType === "a2ui-surface");
    const content = surfaces.at(-1)?.event.content as {
      a2ui_operations: Record<string, unknown>[];
    };
    return content.a2ui_operations;
  };
  const componentsOf = (ops: Record<string, unknown>[]) =>
    (
      ops.find((o) => "updateComponents" in o) as {
        updateComponents: { components: Record<string, unknown>[] };
      }
    ).updateComponents.components;

  it("sends one agent message and one surface of the web's catalog, with Cards and Mermaid, and finishes", async () => {
    const { threadId, body } = await startThread("cards-mermaid please", "reviewer", {
      forwardedProps: { [UI_CATALOG_PROP]: OWN_CATALOG },
    });
    await validated(body, "cards-mermaid run");
    expect(body.at(-1)?.event).toMatchObject({
      type: "RUN_FINISHED",
      outcome: { type: "success" },
    });
    const words = body.filter((f) => f.event.type === "TEXT_MESSAGE_CONTENT");
    expect(JSON.stringify(words)).toContain("I compared three ways to keep a login session");

    const ops = surfaceOf(body);
    expect(ops[0]).toMatchObject({
      createSurface: { surfaceId: "s1", catalogId: OWN_CATALOG.catalogId },
    });
    const components = componentsOf(ops);
    expect(components.map((c) => c.component)).toEqual(["Column", "Text", "Cards", "Mermaid"]);
    const cards = components.find((c) => c.component === "Cards") as {
      cards: Record<string, unknown>[];
    };
    expect(cards.cards).toHaveLength(3);
    expect(cards.cards[2]?.url).toBeUndefined();
    const graph = components.find((c) => c.component === "Mermaid") as { code: string };
    expect(graph.code.startsWith("flowchart TD")).toBe(true);
    await waitForState(threadId, ["done"]);
  });

  it("every component of every mock surface is a component of the catalog the web ships", async () => {
    const names = new Set(Object.keys(OWN_CATALOG.catalog.components));
    for (const word of [
      "cards-mermaid",
      "cards-bad",
      "cards-bad-url",
      "mermaid-bad",
      "mermaid-hostile",
    ]) {
      const { body } = await startThread(`${word} please`, "reviewer", {
        forwardedProps: { [UI_CATALOG_PROP]: OWN_CATALOG },
      });
      for (const c of componentsOf(surfaceOf(body))) {
        expect(names.has(c.component as string), `${word}: ${String(c.component)}`).toBe(true);
      }
    }
  });
});

describe("holds (mock only): a run that waits for the test, and cuts that touch one thread", () => {
  // a server of its own: the sender's refresh comes round every 40 ms here, not every second
  const held = createMockServer({ stepMs: 5, keepaliveMs: 50, refreshMs: 40 });
  let at = "";
  beforeAll(async () => {
    await new Promise<void>((r) => held.listen(0, "127.0.0.1", r));
    // nosemgrep: opt.opengrep-rules.typescript.react.security.react-insecure-request -- loopback test server, never leaves the runner
    at = `http://127.0.0.1:${(held.address() as AddressInfo).port}`;
  });
  afterAll(async () => {
    held.closeAllConnections();
    await new Promise<void>((r) => held.close(() => r()));
  });

  const sleep = (ms: number) => new Promise<"quiet">((r) => setTimeout(() => r("quiet"), ms));

  /** A response as one frame at a time: a frame, `quiet` when none came within `ms`, `end` when it is over. */
  function reader(res: Response, ms = 5000) {
    const stop = new AbortController();
    const gen = readSse(res.body as ReadableStream<Uint8Array>, stop.signal);
    let pending: Promise<IteratorResult<SseFrame>> | undefined;
    return {
      async next(within = ms): Promise<Frame | "quiet" | "end"> {
        pending ??= gen.next();
        const got = await Promise.race([
          pending.catch(() => ({ done: true }) as const),
          sleep(within),
        ]);
        if (got === "quiet") return got;
        pending = undefined;
        if (got.done || !("value" in got) || !got.value) return "end";
        return { event: JSON.parse(got.value.data) as Record<string, unknown> };
      },
      close: () => stop.abort(),
    };
  }

  const frameOf = async (r: ReturnType<typeof reader>, within?: number) => {
    const f = await r.next(within);
    if (f === "quiet" || f === "end") throw new Error(`no frame: the stream is ${f}`);
    return f;
  };

  /** The words of the live pieces of a reply, up to the frame that says `until`. */
  async function said(r: ReturnType<typeof reader>, until: string): Promise<string> {
    let text = "";
    while (!text.includes(until)) {
      const f = await frameOf(r);
      const live = (f.event.metadata as Record<string, unknown> | undefined)?.["vymalo.live"];
      if (f.event.type === "TEXT_MESSAGE_CONTENT" && live) text += String(f.event.delta);
    }
    return text;
  }

  const run = (text: string, threadId = newId()) =>
    postRun(at, "adam", {
      threadId,
      runId: "run-1",
      // not `m-1`: the mock numbers the agent's messages that way, and a log that has a message of that id has said the reply
      messages: [{ id: "user-1", role: "user", content: text }],
    }).then((res) => ({ threadId, res }));
  const release = (threadId: string) =>
    fetch(`${at}/__mock/release?thread=${threadId}`, { method: "POST" });

  it("stream-gate: five pieces and nothing more until released; a joiner is told the text so far; then the rest and the log's message", async () => {
    const { threadId, res } = await run("stream-gate write the plan");
    const writer = reader(res);
    const first = await said(writer, "fix the off-by-one in the loop");
    expect(first.startsWith("I'll start with the failing test")).toBe(true);
    // held: the sender says the text again from the start every 40 ms, and the open reply has it all
    expect(await writer.next(300)).toBe("quiet");

    // a page that opens now heard none of the pieces: the sender's refresh tells it the text so far
    const joiner = reader(await connect(at, threadId));
    expect(await said(joiner, "fix the off-by-one in the loop")).toBe(first);
    expect(await joiner.next(200)).toBe("quiet");

    expect((await release(threadId)).status).toBe(204);
    // goes on: the other three pieces, then the log's message (the rest of the words, final) and done
    const rest: Frame[] = [];
    for (;;) {
      const f = await frameOf(writer);
      rest.push(f);
      if (isTerminal(f)) break;
    }
    expect(rest.at(-1)?.event).toMatchObject({
      type: "RUN_FINISHED",
      outcome: { type: "success" },
    });
    expect(rest.map((f) => String(f.event.delta ?? "")).join("")).toContain("when it is green.");
    // nothing waits any more
    expect((await release(threadId)).status).toBe(409);
    writer.close();
    joiner.close();
  });

  it("stream-abandon: held after the two pieces, and a release that comes the moment they are seen finds it held", async () => {
    const { threadId, res } = await run("stream-abandon what is the answer");
    const writer = reader(res);
    expect(await said(writer, "The answer is forty-")).toBe("The answer is forty-");
    // no wait: the hold is part of the step before it, so a test that saw the last piece can release
    expect((await release(threadId)).status).toBe(204);
    const rest: Frame[] = [];
    for (;;) {
      const f = await frameOf(writer);
      rest.push(f);
      if (isTerminal(f)) break;
    }
    const ends = rest.filter((f) => f.event.type === "TEXT_MESSAGE_END");
    expect(JSON.stringify(ends[0]?.event)).toContain('"abandoned":true');
    expect(JSON.stringify(rest)).toContain("it is forty-two.");
    expect(rest.at(-1)?.event).toMatchObject({
      type: "RUN_FINISHED",
      outcome: { type: "success" },
    });
    writer.close();
  });

  it("a release for a thread that does not wait is a 409, and a thread that is not there is one too", async () => {
    const { threadId, res } = await run("stream-hold write the plan");
    const writer = reader(res);
    await said(writer, "then make the smallest change");
    // `stream-hold` waits for Stop, not for the test
    expect((await release(threadId)).status).toBe(409);
    expect((await release(newId())).status).toBe(409);
    expect((await release("")).status).toBe(409);
    expect((await fetch(`${at}/api/threads/${threadId}/cancel`, { method: "POST" })).status).toBe(
      202,
    );
    writer.close();
  });

  it("drop-streams with a thread cuts that thread's streams and no other's; without one, every stream", async () => {
    const a = await run("stream-hold write the plan");
    const b = await run("stream-hold write the plan");
    const ra = reader(a.res);
    const rb = reader(b.res);
    // the last piece before the hold: after it, nothing more comes on either stream until cancelled
    await said(ra, "fix the off-by-one in the loop");
    await said(rb, "fix the off-by-one in the loop");

    expect(
      (await fetch(`${at}/__mock/drop-streams?thread=${a.threadId}`, { method: "POST" })).status,
    ).toBe(204);
    // a's stream is cut; b's goes on (quiet: it is held)
    for (let f = await ra.next(); f !== "end"; f = await ra.next()) {
      expect(f, "a's stream ends").not.toBe("quiet");
    }
    expect(await rb.next(300)).toBe("quiet");

    expect((await fetch(`${at}/__mock/drop-streams`, { method: "POST" })).status).toBe(204);
    for (let f = await rb.next(); f !== "end"; f = await rb.next()) {
      expect(f, "b's stream ends").not.toBe("quiet");
    }
    for (const t of [a.threadId, b.threadId])
      await fetch(`${at}/api/threads/${t}/cancel`, { method: "POST" });
  });
});

describe("forking a thread (ADR 0029), as the mock does it", () => {
  type Branches = components["schemas"]["Branches"];
  type Exported = {
    events: { seq: number; kind: string; threadId?: string; data: Record<string, unknown> }[];
  };

  /** A thread of two finished jobs (`echo first`, `echo second`) and its log. */
  async function twoJobs(agent = "adam") {
    const { threadId } = await startThread("echo first", agent);
    await waitForState(threadId, ["done"]);
    const res = await postRun(base, agent, {
      threadId,
      runId: "run-2",
      messages: [
        { id: "m-1", role: "user", content: "echo first" },
        { id: "m-2", role: "user", content: "echo second" },
      ],
    });
    expect(res.status).toBe(200);
    await frames(res);
    await waitForState(threadId, ["done"]);
    return { threadId, log: await logOf(threadId) };
  }

  async function logOf(threadId: string): Promise<Exported["events"]> {
    const res = await fetch(`${base}/api/threads/${threadId}/export`);
    return ((await res.json()) as Exported).events;
  }
  /** An event without the thread it is in: a copy is the parent's event in the fork's log. */
  const bare = (log: Exported["events"]) =>
    log.map(({ threadId: _thread, ...event }) => event as unknown);
  const userSeqs = (log: Exported["events"]) =>
    log.filter((e) => e.kind === "user_message").map((e) => e.seq);

  const fork = (threadId: string, body: unknown) => post(`/api/threads/${threadId}/fork`, body);
  const forkOk = async (threadId: string, body: unknown, status = 201): Promise<Thread> => {
    const res = await fork(threadId, body);
    expect(res.status).toBe(status);
    expect(res.headers.get("location")).toMatch(/^\/api\/threads\/[0-9a-f-]{36}$/);
    return (await expectDocumented("/api/threads/{threadId}/fork", "post", res)) as Thread;
  };
  const branchesOf = async (threadId: string): Promise<Branches> => {
    const res = await fetch(`${base}/api/threads/${threadId}/branches`);
    return (await expectDocumented("/api/threads/{threadId}/branches", "get", res)) as Branches;
  };
  const listIds = async (query = ""): Promise<string[]> => {
    const res = await fetch(`${base}/api/threads?limit=100${query}`);
    return ((await expectDocumented("/api/threads", "get", res)) as Thread[]).map((t) => t.id);
  };

  it("fork from here: a finished thread that holds the turns up to the cut and the marker", async () => {
    const { threadId, log } = await twoJobs();
    const [first, second] = userSeqs(log);
    const forked = await forkOk(threadId, { after: first });
    // the cut is the event before the second message
    const cut = (second ?? 0) - 1;
    expect(forked.forkedFrom).toEqual({ threadId, seq: cut, kind: "fork" });
    expect(forked.state).toBe("done");
    expect(forked.lastSeq).toBe(cut + 1);
    expect(forked.title).toBe("echo first");
    expect(forked.target).toEqual({ agentId: "adam" });
    // the parent is as it was
    const parent = await (await fetch(`${base}/api/threads/${threadId}`)).json();
    expect(parent.lastSeq).toBe(log.length);
    expect(parent.forkedFrom).toBeUndefined();

    // its log is the parent's events with the same seq, then the event that says what it is
    const copy = await logOf(forked.id);
    expect(bare(copy.slice(0, cut))).toEqual(bare(log.slice(0, cut)));
    expect(copy.at(-1)).toMatchObject({
      seq: cut + 1,
      kind: "thread_forked",
      data: { from: { threadId, seq: cut }, kind: "fork", title: "echo first" },
    });

    // a viewer reads the copy, then the marker run and the thread's origin
    const read = await validated(
      await frames(await connect(base, forked.id, { mode: "run" })),
      "fork frames",
    );
    const marker = read.find(
      (f) => f.event.type === "ACTIVITY_SNAPSHOT" && f.event.activityType === "vymalo.fork",
    );
    expect(marker?.event.messageId).toBe(`fork-${cut + 1}`);
    expect(marker?.event.content).toMatchObject({
      from: { threadId, seq: cut },
      kind: "fork",
      title: "echo first",
      target: { agentId: "adam" },
    });
    const last = read.at(-1)?.event;
    expect(last).toMatchObject({ type: "RUN_FINISHED", runId: `run-${cut + 1}` });
    const snapshots = read.filter((f) => f.event.type === "STATE_SNAPSHOT");
    expect(snapshots.at(-1)?.event).toMatchObject({
      snapshot: { thread: { state: "done", forkedFrom: { threadId, seq: cut, kind: "fork" } } },
    });
    // the parent's own frames, before the marker, do not say it
    expect(snapshots[0]?.event).not.toMatchObject({
      snapshot: { thread: { forkedFrom: expect.anything() } },
    });
  });

  it("the end of the log is the cut when no message follows; any event of the turn names it", async () => {
    const { threadId, log } = await twoJobs();
    const [first, second] = userSeqs(log);
    // an event in the middle of the second turn
    const forked = await forkOk(threadId, { after: (second ?? 0) + 1 });
    expect(forked.forkedFrom?.seq).toBe(log.length);
    // an event of the first turn: up to the second message
    const earlier = await forkOk(threadId, { after: (first ?? 0) + 1 });
    expect(earlier.forkedFrom?.seq).toBe((second ?? 0) - 1);
  });

  it("continues with another agent: the target is the fork's, and a message goes to that agent", async () => {
    const { threadId, log } = await twoJobs();
    const forked = await forkOk(threadId, {
      after: log.length,
      target: { agentId: "reviewer" },
    });
    expect(forked.target).toEqual({ agentId: "reviewer" });
    expect(forked.forkedFrom).toMatchObject({ threadId, kind: "fork" });
    const refused = await postRun(base, "adam", {
      threadId: forked.id,
      runId: "run-x",
      messages: [{ id: "m-x", role: "user", content: "echo hello" }],
    });
    expect(refused.status).toBe(409);
    await refused.arrayBuffer();
    const sent = await postRun(base, "reviewer", {
      threadId: forked.id,
      runId: "run-y",
      messages: [{ id: "m-y", role: "user", content: "echo hello" }],
    });
    expect(sent.status).toBe(200);
    await frames(sent);
    expect((await waitForState(forked.id, ["done"])).forkedFrom?.kind).toBe("fork");
    // the actor of the new turn is the new agent
    const turn = (await logOf(forked.id)).filter((e) => e.kind === "agent_status").at(-1);
    expect(turn).toBeDefined();
  });

  it("a repeat of a request with the same id answers the fork it made", async () => {
    const { threadId, log } = await twoJobs();
    const id = newId();
    const made = await forkOk(threadId, { after: log.length, id });
    expect(made.id).toBe(id);
    const again = await forkOk(threadId, { after: log.length, id }, 200);
    expect(again.id).toBe(id);
    expect((await logOf(id)).length).toBe(made.lastSeq);
    // the id of another thread is not a fork of this one
    const other = await startThread("echo");
    await waitForState(other.threadId, ["done"]);
    const clash = await fork(threadId, { after: log.length, id: other.threadId });
    expect(clash.status).toBe(409);
    await expectDocumented("/api/threads/{threadId}/fork", "post", clash);
  });

  it("refuses a turn that is going on with turn_open, and takes it once it has ended", async () => {
    const { threadId } = await startThread("slow task");
    await waitForState(threadId, ["working"]);
    const res = await fork(threadId, { after: 1 });
    expect(res.status).toBe(409);
    const problem = (await expectDocumented("/api/threads/{threadId}/fork", "post", res)) as {
      code?: string;
    };
    expect(problem.code).toBe("turn_open");
    expect((await post(`/api/threads/${threadId}/cancel`)).status).toBe(202);
    await waitForState(threadId, ["cancelled"]);
    const forked = await forkOk(threadId, { after: 1 });
    expect(forked.forkedFrom?.kind).toBe("fork");
  });

  it("a thread waiting for an answer forks as it is", async () => {
    const { threadId } = await startThread("ask which branch");
    await waitForState(threadId, ["blocked"]);
    const forked = await forkOk(threadId, { after: 1 });
    expect(forked.state).toBe("done");
    expect(forked.forkedFrom?.seq).toBe(forked.lastSeq - 1);
  });

  it("answers the documented problems", async () => {
    const { threadId, log } = await twoJobs();
    const [first] = userSeqs(log);
    const bad: [string, unknown, number][] = [
      ["neither after nor replace", {}, 400],
      ["both", { after: 1, replace: 1, text: "x" }, 400],
      ["text with after", { after: 1, text: "x" }, 400],
      ["messageId with after", { after: 1, messageId: "m" }, 400],
      ["replace without text", { replace: first }, 400],
      ["an empty text", { replace: first, text: "" }, 400],
      ["a seq that is not a number", { after: "1" }, 400],
      ["a seq of 0", { after: 0 }, 400],
      ["an unknown member", { after: 1, nope: true }, 400],
      ["an unknown agent", { after: 1, target: { agentId: "nobody" } }, 400],
      [
        "a release of an agent without releases",
        { after: 1, target: { agentId: "reviewer", release: "x" } },
        400,
      ],
      ["an unknown release", { after: 1, target: { agentId: "adam", release: "nope" } }, 400],
      ["an id that is not a UUID", { after: 1, id: "x" }, 400],
      ["a seq past the log", { after: log.length + 1 }, 422],
      ["replace past the log", { replace: log.length + 1, text: "x" }, 422],
      ["replace of an agent's event", { replace: (first ?? 0) + 1, text: "x" }, 422],
    ];
    for (const [label, body, status] of bad) {
      const res = await fork(threadId, body);
      expect(res.status, label).toBe(status);
      await expectDocumented("/api/threads/{threadId}/fork", "post", res);
    }
    const missing = await fork(newId(), { after: 1 });
    expect(missing.status).toBe(404);
    await expectDocumented("/api/threads/{threadId}/fork", "post", missing);
    const notJson = await fetch(`${base}/api/threads/${threadId}/fork`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: "{",
    });
    expect(notJson.status).toBe(400);
    // nothing was written by any of them
    expect((await (await fetch(`${base}/api/threads/${threadId}`)).json()).lastSeq).toBe(
      log.length,
    );
  });

  it("an edit: the thread of the parent's turns before the message, the new message and its job", async () => {
    const { threadId, log } = await twoJobs();
    const [, second] = userSeqs(log);
    const edited = await forkOk(threadId, {
      replace: second,
      text: "echo changed",
      messageId: "m-edit",
    });
    expect(edited.forkedFrom).toEqual({ threadId, seq: (second ?? 0) - 1, kind: "edit" });
    expect(edited.state).toBe("queued");
    const done = await waitForState(edited.id, ["done"]);
    expect(done.target).toEqual({ agentId: "adam" });
    const copy = await logOf(edited.id);
    const cut = (second ?? 0) - 1;
    expect(bare(copy.slice(0, cut))).toEqual(bare(log.slice(0, cut)));
    expect(copy[cut]).toMatchObject({ kind: "thread_forked", data: { kind: "edit" } });
    expect(copy[cut + 1]).toMatchObject({
      seq: cut + 2,
      kind: "user_message",
      data: { text: "echo changed", messageId: "m-edit" },
    });
    expect(copy[cut + 2]).toMatchObject({ kind: "job_started", data: { job: 2 } });
    // the answer is the new message's
    expect(JSON.stringify(copy.slice(cut + 3))).toContain("echo: echo changed");

    // the list leaves an edit out unless asked: it is a branch of a conversation it shows
    expect(await listIds()).not.toContain(edited.id);
    expect(await listIds("&branches=include")).toContain(edited.id);
    expect(await listIds()).toContain(threadId);
    // "fork from here" is a conversation of its own, and is listed
    const own = await forkOk(threadId, { after: log.length });
    expect(await listIds()).toContain(own.id);

    // the viewer reads the copy, the marker and the new job, in order
    const read = await validated(
      await frames(await connect(base, edited.id, { mode: "run" })),
      "edit frames",
    );
    const kinds = read.map((f) =>
      f.event.type === "ACTIVITY_SNAPSHOT" ? String(f.event.activityType) : f.event.type,
    );
    expect(kinds.indexOf("vymalo.fork")).toBeGreaterThan(-1);
    expect(kinds.indexOf("vymalo.fork")).toBeLessThan(kinds.lastIndexOf("TEXT_MESSAGE_START"));
    expect(
      read.some((f) => f.event.type === "TEXT_MESSAGE_CONTENT" && f.event.delta === "echo changed"),
    ).toBe(true);
  });

  it("an edit of the first message copies nothing, and starts at job 2", async () => {
    const { threadId, log } = await twoJobs();
    const [first] = userSeqs(log);
    const edited = await forkOk(threadId, { replace: first, text: "echo other" });
    expect(edited.forkedFrom).toEqual({ threadId, seq: 0, kind: "edit" });
    await waitForState(edited.id, ["done"]);
    const copy = await logOf(edited.id);
    expect(copy[0]).toMatchObject({ seq: 1, kind: "thread_forked", data: { from: { seq: 0 } } });
    expect(copy[1]).toMatchObject({ kind: "user_message", data: { text: "echo other" } });
    expect(copy[2]).toMatchObject({ kind: "job_started", data: { job: 2 } });
  });

  it("branches: the messages that have other versions, the original first and then the edits", async () => {
    const { threadId, log } = await twoJobs();
    const [first, second] = userSeqs(log);
    // a thread nobody edited has no points
    expect(await branchesOf(threadId)).toEqual({ root: threadId, points: [] });

    const e1 = await forkOk(threadId, { replace: second, text: "echo one" });
    const e2 = await forkOk(threadId, { replace: second, text: "echo two" });
    await waitForState(e1.id, ["done"]);
    await waitForState(e2.id, ["done"]);
    const versions = (b: Branches, at: number) => b.points.find((p) => p.seq === at);
    const titleOf = async (id: string) =>
      (await (await fetch(`${base}/api/threads/${id}`)).json()).title;

    const original = await branchesOf(threadId);
    expect(original.root).toBe(threadId);
    expect(original.points).toHaveLength(1);
    expect(versions(original, second ?? 0)).toEqual({
      seq: second,
      index: 0,
      siblings: [
        { threadId, seq: second, title: await titleOf(threadId) },
        { threadId: e1.id, seq: (second ?? 0) + 1, title: await titleOf(e1.id) },
        { threadId: e2.id, seq: (second ?? 0) + 1, title: await titleOf(e2.id) },
      ],
    });
    const second1 = await branchesOf(e1.id);
    expect(second1.root).toBe(threadId);
    // this thread's own message is the edit, at its own seq; the first message is shared, so has no versions
    expect(second1.points.map((p) => [p.seq, p.index])).toEqual([[(second ?? 0) + 1, 1]]);
    expect((await branchesOf(e2.id)).points.map((p) => [p.seq, p.index])).toEqual([
      [(second ?? 0) + 1, 2],
    ]);

    // an edit of an edit is another version of the same message
    const e1log = await logOf(e1.id);
    const e3 = await forkOk(e1.id, {
      replace: userSeqs(e1log).at(-1),
      text: "echo three",
    });
    await waitForState(e3.id, ["done"]);
    const third = await branchesOf(e3.id);
    expect(third.root).toBe(threadId);
    const point = third.points.at(-1);
    expect(point?.siblings.map((s) => s.threadId)).toEqual([threadId, e1.id, e2.id, e3.id]);
    expect(point?.index).toBe(3);
    expect(point?.seq).toBe(second === undefined ? 0 : userSeqs(await logOf(e3.id)).at(-1));
    // and the original now lists four versions
    expect(versions(await branchesOf(threadId), second ?? 0)?.siblings).toHaveLength(4);

    // an edit of the first message is another version of that one, and a point of the first message
    const e4 = await forkOk(threadId, { replace: first, text: "echo zero" });
    await waitForState(e4.id, ["done"]);
    const zero = await branchesOf(e4.id);
    expect(zero.root).toBe(threadId);
    expect(zero.points.map((p) => p.seq)).toEqual([2]);
    expect((await branchesOf(threadId)).points.map((p) => p.seq)).toEqual([first, second]);

    // an edit's fork-from-here is its own conversation
    const own = await forkOk(e1.id, { after: (await logOf(e1.id)).length });
    expect(await branchesOf(own.id)).toEqual({ root: own.id, points: [] });
    const missing = await fetch(`${base}/api/threads/${newId()}/branches`);
    expect(missing.status).toBe(404);
    await expectDocumented("/api/threads/{threadId}/branches", "get", missing);
  });
});

describe("who the session is, and what its roles let it do (ADR 0033), as the mock does it", () => {
  type Me = components["schemas"]["Me"];
  type Problem = { title: string; status: number; detail?: string; code?: string };

  let sessions = 0;
  /** A session of its own (the cookie the web carries) that is `me`: the hook, then the headers. */
  async function as(me: "user" | "admin" | "read-only" | "limited" | "no-access") {
    const session = `roles-${++sessions}`;
    expect((await post(`/__mock/config?me=${me}&session=${session}`)).status).toBe(204);
    return { Cookie: `mock-registry=${session}` };
  }
  const get = (p: string, headers: Record<string, string>) => fetch(base + p, { headers });
  const send = (method: string, p: string, headers: Record<string, string>, body?: unknown) =>
    fetch(base + p, {
      method,
      headers: { "Content-Type": "application/json", ...headers },
      ...(body === undefined ? {} : { body: JSON.stringify(body) }),
    });
  /** A thread of `headers`' session, run to its end, and who owns it. */
  async function threadOf(headers: Record<string, string>, text = "echo roles", agent = "adam") {
    const threadId = newId();
    const res = await fetch(`${base}/agui/agents/${agent}`, {
      method: "POST",
      headers: { "Content-Type": "application/json", Accept: "text/event-stream", ...headers },
      body: JSON.stringify({
        state: {},
        tools: [],
        context: [],
        forwardedProps: {},
        threadId,
        runId: "run-1",
        messages: [{ id: "m-1", role: "user", content: text }],
      }),
    });
    expect(res.status).toBe(200);
    await frames(res);
    return threadId;
  }
  const problemOf = async (
    template: string,
    method: string,
    res: Response,
    status: number,
    code?: string,
  ): Promise<Problem> => {
    expect(res.status).toBe(status);
    const body = (await expectDocumented(template, method, res)) as Problem;
    expect(body.code).toBe(code);
    return body;
  };

  it("getMe: the default session is the person every session was before roles", async () => {
    const res = await fetch(`${base}/api/me`);
    expect(res.status).toBe(200);
    expect(res.headers.get("cache-control")).toBe("no-store");
    const me = (await expectDocumented("/api/me", "get", res)) as Me;
    expect(me).toMatchObject({ user: "dev@example.com", roles: ["user"] });
    expect(me.permissions).toContainEqual({ permission: "thread.write", scope: "own" });
    expect(me.agents).toEqual({ read: ["*"], invoke: ["*"] });
  });

  it("getMe: each profile of the hook says what its roles grant, and the scope only where there is one", async () => {
    const seen: Record<string, Me> = {};
    for (const name of ["user", "admin", "read-only", "no-access"] as const) {
      const res = await get("/api/me", await as(name));
      seen[name] = (await expectDocumented("/api/me", "get", res)) as Me;
    }
    const scopes = (me: Me) =>
      Object.fromEntries(me.permissions.map((p) => [p.permission, p.scope ?? null]));
    // an administrator holds `admin` and reaches the threads a user reaches: their own (ADR 0039)
    expect(scopes(seen.admin as Me)).toMatchObject({
      admin: null,
      "thread.read": "own",
      "thread.write": "own",
      "artifact.read": "own",
      "agent.invoke": null,
    });
    // a read-only role: no write, and no agent it may invoke
    expect(scopes(seen["read-only"] as Me)).not.toHaveProperty("thread.write");
    expect(seen["read-only"]?.agents).toEqual({ read: ["*"], invoke: [] });
    // nothing granted: empty, and still a 200
    expect(seen["no-access"]).toMatchObject({ roles: [], permissions: [] });
    expect(new Set(Object.values(seen).map((m) => m.user)).size).toBe(4);
  });

  it("the hook refuses a profile it does not have, and the default is back after a reset", async () => {
    expect((await post("/__mock/config?me=root&session=roles-bad")).status).toBe(400);
    const cookie = await as("admin");
    expect(((await (await get("/api/me", cookie)).json()) as Me).user).toBe("admin@example.com");
    expect((await post("/__mock/reset")).status).toBe(204);
    expect(((await (await get("/api/me", cookie)).json()) as Me).user).toBe("dev@example.com");
  });

  it("no access: every route but getMe is a 403 no_access, and getMe still says who", async () => {
    const nobody = await as("no-access");
    const id = await threadOf({});
    const refused: [string, string, string][] = [
      ["/api/agents", "get", "/api/agents"],
      ["/api/registry", "get", "/api/registry"],
      ["/api/config", "get", "/api/config"],
      ["/api/threads", "get", "/api/threads"],
      ["/api/threads/{threadId}", "get", `/api/threads/${id}`],
      ["/agui/threads/{threadId}/connect", "get", `/agui/threads/${id}/connect`],
    ];
    for (const [template, method, p] of refused) {
      const body = await problemOf(template, method, await get(p, nobody), 403, "no_access");
      expect(body.detail).toBe("your roles do not grant access to this API");
    }
    const run = await fetch(`${base}/agui/agents/adam`, {
      method: "POST",
      headers: { "Content-Type": "application/json", ...nobody },
      body: JSON.stringify({
        threadId: newId(),
        runId: "r",
        messages: [{ id: "m", role: "user", content: "echo" }],
      }),
    });
    await problemOf("/agui/agents/{agentId}", "post", run, 403, "no_access");
    const me = (await (await get("/api/me", nobody)).json()) as Me;
    expect(me.user).toBe("nobody@example.com");
  });

  it("a thread is its maker's: the owner is the session's person, and the list and the thread are theirs alone", async () => {
    const admin = await as("admin");
    const mine = await threadOf({}, "echo mine");
    const theirs = await threadOf(admin, "echo theirs");
    const owned = async (id: string, headers: Record<string, string> = {}) =>
      ((await (await get(`/api/threads/${id}`, headers)).json()) as Thread).owner;
    expect(await owned(mine)).toBe("dev@example.com");
    expect(await owned(theirs, admin)).toBe("admin@example.com");

    const listed = async (headers: Record<string, string>, query = "") =>
      (await expectDocumented(
        "/api/threads",
        "get",
        await get(`/api/threads?limit=100${query}`, headers),
      )) as Thread[];
    const own = await listed({});
    expect(own.map((t) => t.id)).toContain(mine);
    expect(own.map((t) => t.id)).not.toContain(theirs);
    expect(own.every((t) => t.owner === "dev@example.com")).toBe(true);
    // an administrator's plain list is their own too
    expect((await listed(admin)).map((t) => t.id)).toEqual([theirs]);

    // another's thread does not exist for someone who may not read it
    const hidden = await get(`/api/threads/${theirs}`, {});
    await problemOf("/api/threads/{threadId}", "get", hidden, 404);
    await problemOf(
      "/agui/threads/{threadId}/connect",
      "get",
      await get(`/agui/threads/${theirs}/connect`, {}),
      404,
    );
    await problemOf(
      "/api/threads/{threadId}/export",
      "get",
      await get(`/api/threads/${theirs}/export`, {}),
      404,
    );
  });

  it("listThreads with owner: a 400 for everyone, the caller's own address and `*` included (ADR 0039)", async () => {
    const admin = await as("admin");
    await threadOf({}, "echo a");
    await threadOf(admin, "echo b");
    const viewer = await as("read-only");
    for (const headers of [admin, viewer, {}]) {
      for (const owner of ["*", "admin@example.com", "dev@example.com", "", " "]) {
        const res = await get(`/api/threads?limit=100&owner=${encodeURIComponent(owner)}`, headers);
        const body = await problemOf("/api/threads", "get", res, 400);
        expect(body.detail).toBe("owner is not supported (ADR 0039)");
      }
    }
    // and without it, each person's list is their own
    const own = (await (await get("/api/threads?limit=100", admin)).json()) as Thread[];
    expect(own.every((t) => t.owner === "admin@example.com")).toBe(true);
  });

  it("an administrator does not read another's thread, nor change it: 404 on every read, rename, cancel, fork and run", async () => {
    const admin = await as("admin");
    const theirs = await threadOf({}, "echo theirs");
    const reads: [string, string][] = [
      ["/api/threads/{threadId}", `/api/threads/${theirs}`],
      ["/api/threads/{threadId}/export", `/api/threads/${theirs}/export`],
      ["/api/threads/{threadId}/branches", `/api/threads/${theirs}/branches`],
      ["/agui/threads/{threadId}/connect", `/agui/threads/${theirs}/connect?mode=run`],
    ];
    for (const [template, path] of reads) {
      await problemOf(template, "get", await get(path, admin), 404);
    }
    const refused: [string, string, Response][] = [
      [
        "/api/threads/{threadId}",
        "patch",
        await send("PATCH", `/api/threads/${theirs}`, admin, { title: "mine now" }),
      ],
      [
        "/api/threads/{threadId}/cancel",
        "post",
        await send("POST", `/api/threads/${theirs}/cancel`, admin),
      ],
      [
        "/api/threads/{threadId}/fork",
        "post",
        await send("POST", `/api/threads/${theirs}/fork`, admin, { after: 1 }),
      ],
      [
        "/agui/agents/{agentId}",
        "post",
        await send("POST", "/agui/agents/adam", admin, {
          threadId: theirs,
          runId: "run-x",
          messages: [{ id: "m-x", role: "user", content: "echo more" }],
        }),
      ],
    ];
    for (const [template, method, res] of refused) {
      await problemOf(template, method, res, 404);
    }
    // nothing was written: the title is the maker's
    expect(((await (await get(`/api/threads/${theirs}`, {})).json()) as Thread).title).not.toBe(
      "mine now",
    );
    // and the administrator's own thread is theirs to change
    const own = await threadOf(admin, "echo own");
    const renamed = await send("PATCH", `/api/threads/${own}`, admin, { title: "renamed" });
    expect(renamed.status).toBe(200);
    expect(((await renamed.json()) as Thread).owner).toBe("admin@example.com");
  });

  it("a role without thread.write or agent.invoke is a 403 forbidden for what it asks, thread or not", async () => {
    const viewer = await as("read-only");
    const id = await threadOf({}, "echo viewed");
    // the viewer reads their own threads: hand the thread over, as a test can
    expect((await post(`/__mock/owner?thread=${id}&owner=viewer@example.com`)).status).toBe(204);
    const read = await get(`/api/threads/${id}`, viewer);
    expect(((await expectDocumented("/api/threads/{threadId}", "get", read)) as Thread).owner).toBe(
      "viewer@example.com",
    );
    expect((await get("/api/agents", viewer)).status).toBe(200);

    const refused: [string, string, Response][] = [
      [
        "/api/threads/{threadId}",
        "patch",
        await send("PATCH", `/api/threads/${id}`, viewer, { title: "x" }),
      ],
      [
        "/api/threads/{threadId}/cancel",
        "post",
        await send("POST", `/api/threads/${id}/cancel`, viewer),
      ],
      [
        "/api/threads/{threadId}/fork",
        "post",
        await send("POST", `/api/threads/${id}/fork`, viewer, { after: 1 }),
      ],
    ];
    for (const [template, method, res] of refused) {
      const body = await problemOf(template, method, res, 403, "forbidden");
      expect(body.detail).toBe("your roles do not grant thread.write");
    }
    // the same answer for a thread that is not there: it says nothing of what exists
    const nothing = await send("PATCH", `/api/threads/${newId()}`, viewer, { title: "x" });
    await problemOf("/api/threads/{threadId}", "patch", nothing, 403, "forbidden");
    // a run is refused for the write, a person who may write for the agent
    const run = await send("POST", "/agui/agents/adam", viewer, {
      threadId: newId(),
      runId: "run-x",
      messages: [{ id: "m-x", role: "user", content: "echo" }],
    });
    await problemOf("/agui/agents/{agentId}", "post", run, 403, "forbidden");
  });

  it("an agent the roles do not name is a 403 for a run and a fork's target, though they read it and own the thread", async () => {
    const limited = await as("limited");
    expect(((await (await get("/api/me", limited)).json()) as Me).agents).toEqual({
      read: ["*"],
      invoke: ["reviewer"],
    });
    // they read all three agents, and invoke the reviewer
    expect(await (await get("/api/agents", limited)).json()).toHaveLength(3);
    expect((await get("/agui/agents/adam/capabilities", limited)).status).toBe(200);

    const run = (agent: string, threadId: string) =>
      send("POST", `/agui/agents/${agent}`, limited, {
        threadId,
        runId: "run-x",
        messages: [{ id: `m-${threadId}`, role: "user", content: "echo more" }],
      });
    // the reviewer is theirs to start; the coder is a 403 that names the permission and the agent
    const reviewed = await threadOf(limited, "echo review", "reviewer");
    const refusedRun = await run("adam", newId());
    const body = await problemOf("/agui/agents/{agentId}", "post", refusedRun, 403, "forbidden");
    expect(body.detail).toBe("your roles do not grant agent.invoke for the agent adam");

    // a thread of the coder's that is theirs: they read it, and rename it (thread.write is theirs),
    // but writing to it starts the coder, and so does forking it
    const coders = await threadOf({}, "echo coder");
    expect((await post(`/__mock/owner?thread=${coders}&owner=limited@example.com`)).status).toBe(
      204,
    );
    expect((await get(`/api/threads/${coders}`, limited)).status).toBe(200);
    expect((await send("PATCH", `/api/threads/${coders}`, limited, { title: "mine" })).status).toBe(
      200,
    );
    await problemOf("/agui/agents/{agentId}", "post", await run("adam", coders), 403, "forbidden");
    const fork = await send("POST", `/api/threads/${coders}/fork`, limited, { after: 1 });
    await problemOf("/api/threads/{threadId}/fork", "post", fork, 403, "forbidden");
    const toReviewer = await send("POST", `/api/threads/${coders}/fork`, limited, {
      after: 1,
      target: { agentId: "reviewer" },
    });
    expect(toReviewer.status).toBe(201);
    await toReviewer.text();
    // and the reviewer's thread goes on
    for (let i = 0; i < 200; i++) {
      const state = ((await (await get(`/api/threads/${reviewed}`, limited)).json()) as Thread)
        .state;
      if (state === "done") break;
      await new Promise((r) => setTimeout(r, 10));
    }
    expect((await run("reviewer", reviewed)).status).toBe(200);
  });
});

describe("a message sent while the agent works (ADR 0036)", () => {
  const release = (id: string) => post(`/__mock/release?thread=${id}`);
  const lastSeq = async (id: string) =>
    ((await (await fetch(`${base}/api/threads/${id}`)).json()) as Thread).lastSeq;
  const untilSeq = async (id: string, n: number) => {
    for (let i = 0; i < 400 && (await lastSeq(id)) < n; i++)
      await new Promise((r) => setTimeout(r, 10));
  };
  /** A thread whose agent is held at a step, the response of its first run still open. */
  async function working(text: string) {
    const threadId = newId();
    const first = await postRun(base, "reviewer", {
      threadId,
      runId: "run-1",
      messages: [{ id: "m-1", role: "user", content: text }],
    });
    expect(first.status).toBe(200);
    await waitForState(threadId, ["working"]);
    return { threadId, first };
  }
  const messagesOf = (threadId: string, text: string, run = "run-2") => ({
    threadId,
    runId: run,
    messages: [{ id: "m-2", role: "user" as const, content: text }],
  });

  it("serves a steer as a run of its own, and ends the run that was open at the message", async () => {
    const { threadId, first } = await working("gate hold");
    const second = await postRun(base, "reviewer", {
      ...messagesOf(threadId, "echo hurry"),
      forwardedProps: { "vymalo.send": "steer" },
    });
    await expectDocumented("/agui/agents/{agentId}", "post", second);
    const open = await validated(await frames(first), "first response");
    expect(open.at(-1)?.event).toMatchObject({
      type: "RUN_FINISHED",
      runId: "run-1",
      outcome: { type: "success" },
    });
    expect(
      open.some(
        (f) =>
          f.event.type === "SUBAGENT_FINISHED" &&
          (f.event.outcome as { type?: string } | undefined)?.type === "suspended",
      ),
    ).toBe(true);
    await release(threadId);
    const mine = await validated(await frames(second), "second response");
    expect(mine[0]?.event).toMatchObject({ type: "RUN_STARTED", runId: "run-2" });
    expect(types(mine).filter((t) => t === "TEXT_MESSAGE_START")).toEqual([]);
    // the message reaches the agent after its turn: job 2, in a run of its own
    expect(mine.at(-1)?.event).toMatchObject({ type: "RUN_FINISHED", runId: "run-2" });
    await untilSeq(threadId, 11);
    const all = await frames(await connect(base, threadId, { mode: "run" }));
    const sent = all.find(
      (f) => f.event.type === "TEXT_MESSAGE_START" && f.event.messageId === "m-2",
    );
    expect(sent?.event.metadata).toMatchObject({ "vymalo.delivery": "steer" });
  });

  it("serves an interrupt in the run of the message: the cancelled task, then the next job", async () => {
    const { threadId, first } = await working("slow work");
    const second = await postRun(base, "reviewer", {
      ...messagesOf(threadId, "echo do X instead"),
      forwardedProps: { "vymalo.send": "interrupt" },
    });
    const open = await frames(first);
    expect(open.at(-1)?.event).toMatchObject({ type: "RUN_FINISHED", runId: "run-1" });
    const mine = await validated(await frames(second), "second response");
    expect(mine[0]?.event).toMatchObject({ type: "RUN_STARTED", runId: "run-2" });
    expect(mine.at(-1)?.event).toMatchObject({
      type: "RUN_FINISHED",
      runId: "run-2",
      outcome: { type: "success" },
    });
    const statuses = mine
      .filter((f) => f.event.activityType === "vymalo.status")
      .map((f) => (f.event.content as { status: string }).status);
    expect(statuses).toEqual(["canceled", "working", "completed"]);
    expect(
      mine.some(
        (f) => (f.event.snapshot as { thread?: { state?: string } })?.thread?.state === "cancelled",
      ),
    ).toBe(false);
  });

  it("is a 409 without the member, and a 400 with another value, before anything is written", async () => {
    const { threadId, first } = await working("gate hold");
    const plain = await postRun(base, "reviewer", messagesOf(threadId, "hurry"));
    expect(plain.status).toBe(409);
    const body = (await expectDocumented("/agui/agents/{agentId}", "post", plain)) as {
      detail: string;
    };
    expect(body.detail).toContain("vymalo.send");
    for (const bad of ["stop", true, ["steer"]]) {
      const res = await postRun(base, "reviewer", {
        ...messagesOf(threadId, "hurry"),
        forwardedProps: { "vymalo.send": bad },
      });
      expect(res.status).toBe(400);
      await expectDocumented("/agui/agents/{agentId}", "post", res);
    }
    expect(await lastSeq(threadId)).toBe(2);
    await release(threadId);
    await frames(first);
  });
});

describe("MCP servers attached to a thread (ADR 0024), as the mock does it", () => {
  type ToolServer = components["schemas"]["ToolServer"];
  type ThreadTools = components["schemas"]["ThreadTools"];
  type Problem = { title: string; status: number; detail?: string; code?: string };
  type Exported = {
    thread: Thread;
    events: { seq: number; kind: string; actor: { type: string; name: string }; data: unknown }[];
  };

  let sessions = 0;
  /** A session of its own, as the web carries it (a cookie): `me`, and the deployment's servers. */
  async function session(opts: { me?: string; servers?: ToolServer[] } = {}) {
    const name = `tools-${++sessions}`;
    if (opts.me)
      expect((await post(`/__mock/config?me=${opts.me}&session=${name}`)).status).toBe(204);
    if (opts.servers) {
      expect((await post(`/__mock/tool-servers?session=${name}`, opts.servers)).status).toBe(204);
    }
    return { Cookie: `mock-registry=${name}` };
  }
  const send = (method: string, p: string, headers: Record<string, string>, body?: unknown) =>
    fetch(base + p, {
      method,
      headers: { "Content-Type": "application/json", ...headers },
      ...(body === undefined ? {} : { body: JSON.stringify(body) }),
    });
  const put = (threadId: string, headers: Record<string, string>, body: unknown) =>
    send("PUT", `/api/threads/${threadId}/tools`, headers, body);

  /** A thread of `headers`' session, run to its end; `tools` ride the run that creates it. */
  async function threadOf(
    headers: Record<string, string>,
    opts: { agent?: string; tools?: unknown; text?: string } = {},
  ) {
    const threadId = newId();
    const res = await fetch(`${base}/agui/agents/${opts.agent ?? "adam"}`, {
      method: "POST",
      headers: { "Content-Type": "application/json", Accept: "text/event-stream", ...headers },
      body: JSON.stringify({
        state: {},
        tools: [],
        context: [],
        forwardedProps: opts.tools === undefined ? {} : { "vymalo.tools": opts.tools },
        threadId,
        runId: "run-1",
        messages: [{ id: "m-1", role: "user", content: opts.text ?? "echo tools" }],
      }),
    });
    return { threadId, res };
  }
  async function finished(
    headers: Record<string, string>,
    opts: Parameters<typeof threadOf>[1] = {},
  ) {
    const { threadId, res } = await threadOf(headers, opts);
    expect(res.status).toBe(200);
    await frames(res);
    for (let i = 0; i < 200; i++) {
      const t = (await (
        await fetch(`${base}/api/threads/${threadId}`, { headers })
      ).json()) as Thread;
      if (t.state === "done") break;
      await new Promise((r) => setTimeout(r, 10));
    }
    return threadId;
  }
  const exported = async (threadId: string, headers: Record<string, string> = {}) =>
    (await (await fetch(`${base}/api/threads/${threadId}/export`, { headers })).json()) as Exported;
  const toolEvents = (log: Exported["events"]) =>
    log.filter((e) => e.kind === "tools_attached" || e.kind === "tools_detached");
  const problemOf = async (
    template: string,
    method: string,
    res: Response,
    status: number,
    code?: string,
  ): Promise<Problem> => {
    expect(res.status).toBe(status);
    const body = (await expectDocumented(template, method, res)) as Problem;
    expect(body.code).toBe(code);
    return body;
  };

  it("listToolServers: the deployment's servers in its order, icons as data: URIs, no URL or credential, never cached", async () => {
    const res = await fetch(`${base}/api/tool-servers`);
    expect(res.status).toBe(200);
    expect(res.headers.get("cache-control")).toBe("no-store");
    const list = (await expectDocumented("/api/tool-servers", "get", res)) as ToolServer[];
    expect(list.map((s) => s.id)).toEqual(["websearch", "github", "docs"]);
    expect(list[0]?.icon).toMatch(/^data:image\/svg\+xml;base64,/);
    expect(list[1]?.icon).toBeUndefined();
    expect(list[1]?.agents).toEqual(["adam"]);
    expect(list[2]?.icon).toMatch(/^data:image\/png;base64,/);
    for (const s of list) {
      expect(
        Object.keys(s).every((k) => ["id", "name", "description", "icon", "agents"].includes(k)),
      ).toBe(true);
    }
    // a test sets the list of its own session; the others keep theirs
    const own = await session({ servers: [{ id: "only", name: "Only one" }] });
    const mine = await fetch(`${base}/api/tool-servers`, { headers: own });
    expect(((await mine.json()) as ToolServer[]).map((s) => s.id)).toEqual(["only"]);
    expect(((await (await fetch(`${base}/api/tool-servers`)).json()) as ToolServer[]).length).toBe(
      3,
    );
    // an empty list is a deployment with nothing to attach
    const none = await session({ servers: [] });
    const empty = await fetch(`${base}/api/tool-servers`, { headers: none });
    expect(await expectDocumented("/api/tool-servers", "get", empty)).toEqual([]);
  });

  it("listToolServers takes thread.write: a role without it is 403 forbidden, one that grants nothing no_access", async () => {
    const viewer = await session({ me: "read-only" });
    const body = await problemOf(
      "/api/tool-servers",
      "get",
      await fetch(`${base}/api/tool-servers`, { headers: viewer }),
      403,
      "forbidden",
    );
    expect(body.detail).toBe("your roles do not grant thread.write");
    const nobody = await session({ me: "no-access" });
    await problemOf(
      "/api/tool-servers",
      "get",
      await fetch(`${base}/api/tool-servers`, { headers: nobody }),
      403,
      "no_access",
    );
  });

  it("putThreadTools: the whole set, sorted, in any state; one tools_attached and one tools_detached for what differs, the same set nothing", async () => {
    const threadId = await finished({});
    const before = (await exported(threadId)).events.length;

    const attach = await put(threadId, {}, { servers: ["docs", "websearch", "docs"] });
    expect(attach.status).toBe(200);
    expect(
      ((await expectDocumented("/api/threads/{threadId}/tools", "put", attach)) as ThreadTools)
        .servers,
    ).toEqual(["docs", "websearch"]);
    let doc = await exported(threadId);
    expect(doc.thread.tools).toEqual(["docs", "websearch"]);
    expect(toolEvents(doc.events)).toEqual([
      expect.objectContaining({
        kind: "tools_attached",
        actor: { type: "user", name: "dev@example.com" },
        data: { servers: ["docs", "websearch"] },
      }),
    ]);
    expect(doc.events.length).toBe(before + 1);

    // the same set writes nothing and is still a 200
    const same = await put(threadId, {}, { servers: ["websearch", "docs"] });
    expect(same.status).toBe(200);
    await same.text();
    expect((await exported(threadId)).events.length).toBe(before + 1);

    // one more and one fewer: an attach and a detach, each with the ids that came or went
    const swap = await put(threadId, {}, { servers: ["websearch", "github"] });
    expect(((await swap.json()) as ThreadTools).servers).toEqual(["github", "websearch"]);
    doc = await exported(threadId);
    expect(
      toolEvents(doc.events)
        .slice(1)
        .map((e) => [e.kind, e.data]),
    ).toEqual([
      ["tools_attached", { servers: ["github"] }],
      ["tools_detached", { servers: ["docs"] }],
    ]);

    // none at all: `tools` is absent from the thread
    const none = await put(threadId, {}, { servers: [] });
    expect(((await none.json()) as ThreadTools).servers).toEqual([]);
    const thread = (await (await fetch(`${base}/api/threads/${threadId}`)).json()) as Thread;
    expect(thread.tools).toBeUndefined();
    expect(thread.state).toBe("done");
  });

  it("the stream says an attach as a snapshot with thread.tools and a vymalo.tools card, in a run of its own for a finished thread", async () => {
    const threadId = await finished({});
    expect((await put(threadId, {}, { servers: ["websearch"] })).status).toBe(200);
    const read = await validated(
      await frames(await connect(base, threadId, { mode: "run" })),
      "tools frames",
    );
    const card = read.find(
      (f) => f.event.type === "ACTIVITY_SNAPSHOT" && f.event.activityType === "vymalo.tools",
    );
    expect(card?.event.content).toMatchObject({ attached: ["websearch"] });
    const snapshots = read.filter((f) => f.event.type === "STATE_SNAPSHOT");
    expect(snapshots.at(-1)?.event).toMatchObject({
      snapshot: { thread: { state: "done", tools: ["websearch"] } },
    });
    // a snapshot from before the attach says no tools
    expect(snapshots[0]?.event).not.toHaveProperty("snapshot.thread.tools");
  });

  it("putThreadTools: 400 for a body that is not exactly {servers: [ids]} or an id that is not a server id, and nothing is written", async () => {
    const threadId = await finished({});
    const bodies: unknown[] = [
      null,
      [],
      "websearch",
      {},
      { servers: "websearch" },
      { servers: [1] },
      { servers: ["websearch"], extra: true },
      { servers: ["Web Search"] },
      { servers: ["-x"] },
      { servers: ["a".repeat(32)] },
      { servers: [""] },
    ];
    for (const body of bodies) {
      await problemOf("/api/threads/{threadId}/tools", "put", await put(threadId, {}, body), 400);
    }
    const notJson = await fetch(`${base}/api/threads/${threadId}/tools`, {
      method: "PUT",
      headers: { "Content-Type": "application/json" },
      body: "{",
    });
    await problemOf("/api/threads/{threadId}/tools", "put", notJson, 400);
    expect(toolEvents((await exported(threadId)).events)).toEqual([]);
  });

  it("putThreadTools: 422 for a server the deployment does not list or does not offer for the thread's agent, or more than 16; the detail names the id", async () => {
    const reviewed = await finished({}, { agent: "reviewer" });
    const unlisted = await problemOf(
      "/api/threads/{threadId}/tools",
      "put",
      await put(reviewed, {}, { servers: ["nope"] }),
      422,
    );
    expect(unlisted.detail).toContain("nope");
    const notForAgent = await problemOf(
      "/api/threads/{threadId}/tools",
      "put",
      await put(reviewed, {}, { servers: ["websearch", "github"] }),
      422,
    );
    expect(notForAgent.detail).toContain("github");
    expect(notForAgent.detail).toContain("reviewer");
    // one refused server attaches nothing: not even the one that was fine
    expect(((await exported(reviewed)).thread.tools ?? []).length).toBe(0);

    const many = Array.from({ length: 17 }, (_, i) => `s${i}`);
    const own = await session({ servers: many.map((id) => ({ id, name: id })) });
    const crowded = await finished(own);
    await problemOf(
      "/api/threads/{threadId}/tools",
      "put",
      await put(crowded, own, { servers: many }),
      422,
    );
    // sixteen are fine
    const sixteen = await put(crowded, own, { servers: many.slice(0, 16) });
    expect(sixteen.status).toBe(200);
    expect(((await sixteen.json()) as ThreadTools).servers.length).toBe(16);
  });

  it("putThreadTools: a server the thread has is not checked again; a person can detach what the deployment stopped listing", async () => {
    const own = await session({
      servers: [
        { id: "websearch", name: "Web search" },
        { id: "files", name: "Files" },
      ],
    });
    const threadId = await finished(own);
    expect((await put(threadId, own, { servers: ["websearch", "files"] })).status).toBe(200);
    // the deployment stops listing `files`
    expect(
      (
        await post(`/__mock/tool-servers?session=${own.Cookie.split("=")[1]}`, [
          { id: "websearch", name: "Web search" },
        ])
      ).status,
    ).toBe(204);
    // keeping it is no new attach: 200; and so is dropping it
    const kept = await put(threadId, own, { servers: ["files", "websearch"] });
    expect(kept.status).toBe(200);
    await kept.text();
    const dropped = await put(threadId, own, { servers: ["websearch"] });
    expect(((await dropped.json()) as ThreadTools).servers).toEqual(["websearch"]);
    // but it cannot be attached again
    await problemOf(
      "/api/threads/{threadId}/tools",
      "put",
      await put(threadId, own, { servers: ["websearch", "files"] }),
      422,
    );
  });

  it("putThreadTools: 404 for a thread that is not there or not the caller's (an administrator's included), 403 forbidden without thread.write", async () => {
    await problemOf(
      "/api/threads/{threadId}/tools",
      "put",
      await put(newId(), {}, { servers: [] }),
      404,
    );
    const theirs = await finished({});
    const stranger = await session({ me: "limited" });
    await problemOf(
      "/api/threads/{threadId}/tools",
      "put",
      await put(theirs, stranger, { servers: ["websearch"] }),
      404,
    );
    const admin = await session({ me: "admin" });
    await problemOf(
      "/api/threads/{threadId}/tools",
      "put",
      await put(theirs, admin, { servers: ["websearch"] }),
      404,
    );
    const viewer = await session({ me: "read-only" });
    await problemOf(
      "/api/threads/{threadId}/tools",
      "put",
      await put(theirs, viewer, { servers: ["websearch"] }),
      403,
      "forbidden",
    );
    expect(toolEvents((await exported(theirs)).events)).toEqual([]);
  });

  it("a run that creates a thread attaches the servers it names, in the creation commit after the message", async () => {
    const { threadId, res } = await threadOf({}, { tools: ["websearch", "docs"] });
    expect(res.status).toBe(200);
    const body = await validated(await frames(res), "creation frames");
    const card = body.find(
      (f) => f.event.type === "ACTIVITY_SNAPSHOT" && f.event.activityType === "vymalo.tools",
    );
    expect(card?.event.content).toMatchObject({ attached: ["docs", "websearch"] });
    const doc = await exported(threadId);
    expect(doc.events.slice(0, 2).map((e) => e.kind)).toEqual(["user_message", "tools_attached"]);
    expect(doc.thread.tools).toEqual(["docs", "websearch"]);
    // the first snapshot has no tools; the one after the attach has them
    const snapshots = body.filter((f) => f.event.type === "STATE_SNAPSHOT");
    expect(snapshots[0]?.event).not.toHaveProperty("snapshot.thread.tools");
    expect(snapshots.at(-1)?.event).toMatchObject({
      snapshot: { thread: { tools: ["docs", "websearch"] } },
    });
  });

  it("vymalo.tools that is [], null or absent attaches none; one that is not an array of strings is a 400 before the stream, a server not offered for the agent a 422, and no thread is made", async () => {
    for (const tools of [[], null, undefined]) {
      const { threadId, res } = await threadOf({}, { tools });
      expect(res.status).toBe(200);
      await frames(res);
      expect((await exported(threadId)).thread.tools).toBeUndefined();
    }
    for (const tools of ["websearch", { id: "websearch" }, [1], [["websearch"]]]) {
      const { threadId, res } = await threadOf({}, { tools });
      await problemOf("/agui/agents/{agentId}", "post", res, 400);
      expect((await fetch(`${base}/api/threads/${threadId}`)).status).toBe(404);
    }
    const refused = await threadOf({}, { tools: ["websearch", "github"], agent: "reviewer" });
    const body = await problemOf("/agui/agents/{agentId}", "post", refused.res, 422);
    expect(body.detail).toContain("github");
    expect((await fetch(`${base}/api/threads/${refused.threadId}`)).status).toBe(404);
    const crowded = await threadOf({}, { tools: Array.from({ length: 17 }, (_, i) => `s${i}`) });
    await problemOf("/agui/agents/{agentId}", "post", crowded.res, 422);
  });

  it("a run on a thread that exists carries vymalo.tools and attaches nothing (the set is changed with PUT)", async () => {
    const threadId = await finished({});
    const res = await postRun(base, "adam", {
      threadId,
      runId: "run-2",
      messages: [
        { id: "m-1", role: "user", content: "echo tools" },
        { id: "m-2", role: "user", content: "echo more" },
      ],
      forwardedProps: { "vymalo.tools": ["websearch"] },
    });
    expect(res.status).toBe(200);
    await frames(res);
    expect(toolEvents((await exported(threadId)).events)).toEqual([]);
  });

  it("a thread keeps its servers from job to job, and a fork keeps what its agent may use and detaches the rest first", async () => {
    const threadId = await finished({}, { tools: ["docs", "github", "websearch"] });
    const next = await postRun(base, "adam", {
      threadId,
      runId: "run-2",
      messages: [
        { id: "m-1", role: "user", content: "echo tools" },
        { id: "m-2", role: "user", content: "echo second" },
      ],
    });
    await frames(next);
    expect(
      ((await (await fetch(`${base}/api/threads/${threadId}`)).json()) as Thread).tools,
    ).toEqual(["docs", "github", "websearch"]);

    const same = await fetch(`${base}/api/threads/${threadId}/fork`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ after: 1 }),
    });
    expect(same.status).toBe(201);
    expect(((await same.json()) as Thread).tools).toEqual(["docs", "github", "websearch"]);

    // to the reviewer, which may use `docs` and `websearch` but not `github`
    const toReviewer = await fetch(`${base}/api/threads/${threadId}/fork`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ after: 1, target: { agentId: "reviewer" } }),
    });
    expect(toReviewer.status).toBe(201);
    const fork = (await expectDocumented(
      "/api/threads/{threadId}/fork",
      "post",
      toReviewer,
    )) as Thread;
    expect(fork.tools).toEqual(["docs", "websearch"]);
    const log = (await exported(fork.id)).events;
    expect(toolEvents(log).at(-1)).toMatchObject({
      kind: "tools_detached",
      data: { servers: ["github"] },
    });
    const forkedAt = log.findIndex((e) => e.kind === "thread_forked");
    expect(log[forkedAt + 1]?.kind).toBe("tools_detached");
  });

  it("capabilities: only the agents whose card lists thread-tools/v1 say so, in custom", async () => {
    const uri = "https://agents.vymalo.com/a2a/extensions/thread-tools/v1";
    const custom = async (agent: string) =>
      (
        (await (await fetch(`${base}/agui/agents/${agent}/capabilities`)).json()) as {
          custom?: Record<string, unknown>;
        }
      ).custom;
    expect(Object.keys((await custom("adam")) ?? {})).toContain(uri);
    expect(Object.keys((await custom("reviewer")) ?? {})).not.toContain(uri);
    expect(Object.keys((await custom("verifier")) ?? {})).not.toContain(uri);
  });

  it("capabilities: only the agents whose card lists steer/v1 say so, in custom (the web words Send by it)", async () => {
    const uri = "https://agents.vymalo.com/a2a/extensions/steer/v1";
    const custom = async (agent: string) =>
      (
        (await (await fetch(`${base}/agui/agents/${agent}/capabilities`)).json()) as {
          custom?: Record<string, unknown>;
        }
      ).custom;
    expect(Object.keys((await custom("adam")) ?? {})).toContain(uri);
    expect(Object.keys((await custom("reviewer")) ?? {})).not.toContain(uri);
  });
});

describe("mentions (ADR 0026, docs/api/mentions-v1.md), as the mock does it", () => {
  type Problem = { title: string; status: number; detail?: string; code?: string };
  type Exported = {
    thread: Thread;
    events: { seq: number; kind: string; data: { text?: string; mentions?: unknown[] } }[];
  };
  let sessions = 0;
  /** A session of its own: a registry, a profile; the headers that carry it and the hook for it. */
  async function session(me?: "user" | "limited") {
    const name = `mentions-${++sessions}`;
    if (me) expect((await post(`/__mock/config?me=${me}&session=${name}`)).status).toBe(204);
    return {
      name,
      headers: { Cookie: `mock-registry=${name}` },
      hook: (p: string, body?: unknown) =>
        post(`${p}${p.includes("?") ? "&" : "?"}session=${name}`, body),
    };
  }
  const run = (
    agent: string,
    headers: Record<string, string>,
    input: {
      threadId?: string;
      runId?: string;
      text: string;
      mentions?: unknown;
      props?: Record<string, unknown>;
      messageId?: string;
    },
  ) => {
    const forwardedProps = {
      ...(input.mentions === undefined ? {} : { "vymalo.mentions": input.mentions }),
      ...input.props,
    };
    return fetch(`${base}/agui/agents/${agent}`, {
      method: "POST",
      headers: { "Content-Type": "application/json", Accept: "text/event-stream", ...headers },
      body: JSON.stringify({
        state: {},
        tools: [],
        context: [],
        forwardedProps,
        threadId: input.threadId ?? newId(),
        runId: input.runId ?? "run-1",
        messages: [{ id: input.messageId ?? "m-1", role: "user", content: input.text }],
      }),
    });
  };
  const ref = (text: string, id: string, extra: Record<string, unknown> = {}) => {
    const label = `@${id}`;
    const start = text.indexOf(label);
    return { agentId: id, label, start, end: start + label.length, ...extra };
  };
  const problemOf = async (res: Response, status: number): Promise<Problem> => {
    expect(res.status).toBe(status);
    return (await expectDocumented("/agui/agents/{agentId}", "post", res)) as Problem;
  };
  const exported = async (threadId: string) =>
    (await (await fetch(`${base}/api/threads/${threadId}/export`)).json()) as Exported;
  const cardOf = async (id: string) =>
    ((await (await fetch(`${base}/api/agents`)).json()) as { id: string; cardUrl?: string }[]).find(
      (a) => a.id === id,
    )?.cardUrl;

  it("records a mention as sent, in UTF-16 code units, and the connect stream says it on the message", async () => {
    const s = await session();
    const text = "😄 ask @adam to plot é";
    const mention = ref(text, "adam", { cardUrl: await cardOf("adam") });
    expect(mention.start).toBe(7); // the emoji is two code units, then " ask "
    const threadId = newId();
    const res = await run("reviewer", s.headers, { threadId, text, mentions: [mention] });
    expect(res.status).toBe(200);
    await validated(await frames(res), "mentions run");
    const log = await exported(threadId);
    const message = log.events.find((e) => e.kind === "user_message");
    expect(message?.data.mentions).toEqual([mention]);
    const all = await frames(await connect(base, threadId, { mode: "run" }));
    const start = all.find((f) => f.event.type === "TEXT_MESSAGE_START" && f.event.role === "user");
    expect(start?.event.metadata).toMatchObject({ "vymalo.mentions": [mention] });
    expect(text.slice(mention.start, mention.end)).toBe("@adam");
  });

  it("a message that mentions nobody has no member, and null or [] mean none", async () => {
    const s = await session();
    for (const mentions of [undefined, null, []]) {
      const threadId = newId();
      const res = await run("reviewer", s.headers, { threadId, text: "echo plain", mentions });
      expect(res.status).toBe(200);
      await frames(res);
      const message = (await exported(threadId)).events.find((e) => e.kind === "user_message");
      expect("mentions" in (message?.data ?? {})).toBe(false);
    }
  });

  it("is a 400 for a shape that is not a list of at most 16 references with exactly their members", async () => {
    const s = await session();
    const text = "ask @adam";
    const ok = ref(text, "adam");
    const sixteen = Array.from({ length: 17 }, () => ok);
    for (const bad of [
      "adam",
      { agentId: "adam" },
      sixteen,
      [{ ...ok, extra: 1 }],
      [{ ...ok, start: 4.5 }],
      [{ ...ok, agentId: 7 }],
      [{ ...ok, cardUrl: 7 }],
      ["@adam"],
    ]) {
      await problemOf(await run("reviewer", s.headers, { text, mentions: bad }), 400);
    }
  });

  it("is a 422 for a label that is not the text, an offset inside a surrogate pair, and references out of order or overlapping", async () => {
    const s = await session();
    const text = "😄 @adam and @verifier";
    const coder = ref(text, "adam");
    const verifier = ref(text, "verifier");
    const cases: [string, unknown][] = [
      ["not the text", [{ ...coder, start: coder.start + 1, end: coder.end + 1 }]],
      ["label without @", [{ ...coder, label: "adam", start: coder.start + 1 }]],
      ["inside a pair", [{ ...coder, start: 1, end: 1 + coder.label.length }]],
      ["past the end", [{ ...coder, end: text.length + 5 }]],
      ["start is not before end", [{ ...coder, end: coder.start }]],
      ["out of order", [verifier, coder]],
      [
        "overlapping",
        [
          coder,
          { ...verifier, start: coder.start + 2, end: coder.start + 2 + verifier.label.length },
        ],
      ],
      ["label too long", [{ agentId: "adam", label: `@${"a".repeat(64)}`, start: 0, end: 65 }]],
    ];
    for (const [name, mentions] of cases) {
      const body = await problemOf(await run("reviewer", s.headers, { text, mentions }), 422);
      expect(body.detail, name).toContain("vymalo.mentions[");
    }
  });

  it("is a 422 for an unknown agent, a card that moved, the thread's own agent and one the roles may not invoke", async () => {
    const s = await session();
    const text = "ask @ghost and @adam";
    const unknown = await problemOf(
      await run("reviewer", s.headers, { text, mentions: [ref(text, "ghost")] }),
      422,
    );
    expect(unknown.detail).toBe("unknown agent 'ghost' in mentions");
    const moved = await problemOf(
      await run("reviewer", s.headers, {
        text,
        mentions: [ref(text, "adam", { cardUrl: "http://elsewhere/card.json" })],
      }),
      422,
    );
    expect(moved.detail).toBe("the card of 'adam' moved; refresh the agent list");
    const own = await problemOf(
      await run("adam", s.headers, { text, mentions: [ref(text, "adam")] }),
      422,
    );
    expect(own.detail).toBe("an agent cannot be mentioned in its own thread");
    // a thread of the reviewer's for a person who may invoke the reviewer only: the coder is not theirs
    const limited = await session("limited");
    const may = await problemOf(
      await run("reviewer", limited.headers, { text, mentions: [ref(text, "adam")] }),
      422,
    );
    expect(may.detail).toBe("you may not use 'adam'");
    // roles first: an id the registry does not list is the same answer, so nothing is learned of the registry
    const none = await problemOf(
      await run("reviewer", limited.headers, { text, mentions: [ref(text, "ghost")] }),
      422,
    );
    expect(none.detail).toBe("you may not use 'ghost'");
  });

  it("a registry's agent can be mentioned, and when the registry cannot answer it is a 503", async () => {
    const s = await session();
    expect((await s.hook("/__mock/registry/agents", { id: "helper", name: "Helper" })).status).toBe(
      204,
    );
    const text = "ask @helper";
    const listed = await run("reviewer", s.headers, { text, mentions: [ref(text, "helper")] });
    expect(listed.status).toBe(200);
    await frames(listed);
    expect((await s.hook("/__mock/registry?down=true")).status).toBe(204);
    const down = await problemOf(
      await run("reviewer", s.headers, { text, mentions: [ref(text, "helper")] }),
      503,
    );
    expect(down.detail).toContain("registry");
    // a configured agent does not need the registry
    const configured = "ask @adam";
    const fine = await run("reviewer", s.headers, {
      text: configured,
      mentions: [ref(configured, "adam")],
    });
    expect(fine.status).toBe(200);
    await frames(fine);
  });

  it("a refusal writes nothing: no thread is made, and an open thread's log does not grow", async () => {
    const s = await session();
    const text = "ask @ghost";
    const threadId = newId();
    await problemOf(
      await run("reviewer", s.headers, { threadId, text, mentions: [ref(text, "ghost")] }),
      422,
    );
    expect((await fetch(`${base}/api/threads/${threadId}`, { headers: s.headers })).status).toBe(
      404,
    );
    const open = newId();
    const first = await run("reviewer", s.headers, { threadId: open, text: "echo hi" });
    await frames(first);
    const before = (await exported(open)).events.length;
    await problemOf(
      await run("reviewer", s.headers, {
        threadId: open,
        runId: "run-2",
        messageId: "m-2",
        text,
        mentions: [ref(text, "ghost")],
      }),
      422,
    );
    expect((await exported(open)).events.length).toBe(before);
  });

  it("a follow-up and a message sent while the agent works keep their mentions", async () => {
    const s = await session();
    const threadId = newId();
    const first = await run("reviewer", s.headers, { threadId, text: "gate hold" });
    expect(first.status).toBe(200);
    for (let i = 0; i < 200; i++) {
      const state = (
        (await (
          await fetch(`${base}/api/threads/${threadId}`, { headers: s.headers })
        ).json()) as Thread
      ).state;
      if (state === "working") break;
      await new Promise((r) => setTimeout(r, 10));
    }
    const text = "echo 😄 and @adam";
    const steered = await run("reviewer", s.headers, {
      threadId,
      runId: "run-2",
      messageId: "m-2",
      text,
      mentions: [ref(text, "adam")],
      props: { "vymalo.send": "steer" },
    });
    expect(steered.status).toBe(200);
    expect((await post(`/__mock/release?thread=${threadId}`)).status).toBe(204);
    await frames(first);
    await frames(steered);
    // the message of the next job has its mentions too, whichever way the log is read
    const log = await exported(threadId);
    const sent = log.events.filter((e) => e.kind === "user_message");
    expect(sent.map((e) => e.data.mentions)).toEqual([undefined, [ref(text, "adam")]]);
    // and a follow-up on the finished thread: the message sent while the agent worked starts a job
    // of its own after the turn (the gate script does not list steer/v1), so the thread is `done`
    // twice; wait for the `done` that ends that job (after its `job_started`), not the first one
    for (let i = 0; i < 400; i++) {
      const kinds = (await exported(threadId)).events.map((e) => e.kind);
      const job2 = kinds.lastIndexOf("job_started");
      const after = job2 < 0 ? [] : kinds.slice(job2);
      const state = (
        (await (
          await fetch(`${base}/api/threads/${threadId}`, { headers: s.headers })
        ).json()) as Thread
      ).state;
      if (state === "done" && after.includes("thread_state")) break;
      await new Promise((r) => setTimeout(r, 10));
    }
    const again = "echo @verifier too";
    const follow = await run("reviewer", s.headers, {
      threadId,
      runId: "run-3",
      messageId: "m-3",
      text: again,
      mentions: [ref(again, "verifier")],
    });
    expect(follow.status).toBe(200);
    await frames(follow);
    const last = (await exported(threadId)).events.filter((e) => e.kind === "user_message").at(-1);
    expect(last?.data.mentions).toEqual([ref(again, "verifier")]);
  });

  it("a run with no message ignores the member (after its shape is checked)", async () => {
    const s = await session();
    const threadId = newId();
    await frames(await run("reviewer", s.headers, { threadId, text: "echo hi" }));
    const cancel = await fetch(`${base}/agui/agents/reviewer`, {
      method: "POST",
      headers: { "Content-Type": "application/json", Accept: "text/event-stream", ...s.headers },
      body: JSON.stringify({
        state: {},
        tools: [],
        context: [],
        forwardedProps: {
          "vymalo.mentions": [{ agentId: "ghost", label: "@ghost", start: 0, end: 6 }],
        },
        threadId,
        runId: "run-9",
        messages: [],
      }),
    });
    // nothing to run is the mock's own 422, not the mention's: the reference is not read against a text
    const body = (await cancel.json()) as Problem;
    expect(body.detail ?? "").not.toContain("ghost");
  });

  it("capabilities: the coder and the verifier list mentions/v1, the reviewer does not; the coder alone lists thread-tools/v1", async () => {
    const uri = "https://agents.vymalo.com/a2a/extensions/mentions/v1";
    const custom = async (agent: string) =>
      Object.keys(
        (
          (await (await fetch(`${base}/agui/agents/${agent}/capabilities`)).json()) as {
            custom?: Record<string, unknown>;
          }
        ).custom ?? {},
      );
    expect(await custom("adam")).toContain(uri);
    expect(await custom("verifier")).toContain(uri);
    expect(await custom("reviewer")).not.toContain(uri);
    const tools = "https://agents.vymalo.com/a2a/extensions/thread-tools/v1";
    expect(await custom("verifier")).not.toContain(tools);
    expect(await custom("adam")).toContain(tools);
  });
});

describe("asked agents (ADR 0026, ask_agent), as the mock plays them", () => {
  type Ev = Record<string, unknown>;
  const kinds = (list: Frame[]) => list.map((f) => f.event as Ev);
  const asks = (list: Frame[]) =>
    kinds(list)
      .filter((e) => e.activityType === "vymalo.ask")
      .map(
        (e): Ev => ({
          id: e.messageId,
          run: e.subagentRunId,
          ...(e.content as Ev),
        }),
      );
  const subagents = (list: Frame[], type: string) =>
    kinds(list)
      .filter((e) => e.type === type)
      .map((e) => e.subagentRunId);

  it("ask-agent: the reviewer, the verifier it asks under it, both answer; then a failed ask, all validated", async () => {
    const { threadId, body } = await startThread("ask-agent coordinate the review");
    expect(body.at(-1)?.event).toMatchObject({
      type: "RUN_FINISHED",
      outcome: { type: "success" },
    });
    // the asked agents are subagents named after them, nested by parentSubagentRunId
    const started = kinds(body).filter((e) => e.type === "SUBAGENT_STARTED");
    const invocation = started[0]?.subagentRunId;
    expect(started.map((e) => [e.subagentRunId, e.name, e.parentSubagentRunId])).toEqual([
      [invocation, "adam", undefined],
      ["sub-ask-1", "reviewer", invocation],
      ["sub-ask-2", "verifier", "sub-ask-1"],
      ["sub-ask-3", "verifier", invocation],
    ]);
    // children end before their parents; the one that failed ends in an error
    const ends = kinds(body)
      .filter((e) => e.type === "SUBAGENT_FINISHED" || e.type === "SUBAGENT_ERROR")
      .map((e) => [e.type, e.subagentRunId, (e.result as Ev | undefined)?.state ?? e.code]);
    expect(ends).toEqual([
      ["SUBAGENT_FINISHED", "sub-ask-2", "completed"],
      ["SUBAGENT_FINISHED", "sub-ask-1", "completed"],
      ["SUBAGENT_ERROR", "sub-ask-3", "ask_failed"],
      ["SUBAGENT_FINISHED", invocation, undefined],
    ]);
    // the activity: one id per ask, said again at its end, attributed to the subagent that asked
    const told = asks(body);
    expect(told.map((a) => [a.id, a.state, a.run])).toEqual([
      ["ask-1", "running", invocation],
      ["ask-2", "running", "sub-ask-1"],
      ["ask-2", "completed", "sub-ask-1"],
      ["ask-1", "completed", invocation],
      ["ask-3", "running", invocation],
      ["ask-3", "failed", invocation],
    ]);
    expect(told[1]).toMatchObject({
      agent: "verifier",
      by: "ask:1",
      depth: 2,
      parentStepId: "ask-1",
      stepId: "ask-2",
    });
    expect(told[2]).toMatchObject({
      answer: "The claims hold: the sources agree with the plan.",
      artifacts: [{ name: "sources" }],
    });
    expect(told[5]).toMatchObject({ error: "the verifier did not answer: connection refused" });
    // the search the verifier relayed is a step of its ask: path ask-2, in sub-ask-2
    const relayed = kinds(body).find(
      (e) => e.activityType === "vymalo.step" && (e.content as Ev).id === "tool-ask-2a",
    );
    expect(relayed).toMatchObject({
      subagentRunId: "sub-ask-2",
      content: { path: ["ask-2"], state: "running" },
    });
    expect((await waitForState(threadId, ["done"])).state).toBe("done");
    // the log carries them as events (the export), and a viewer that connects later reads the same asks
    const exported = (await (await fetch(`${base}/api/threads/${threadId}/export`)).json()) as {
      events: { kind: string; data: Ev; actor: { name: string } }[];
    };
    expect(
      exported.events
        .filter((e) => e.kind === "ask_started" || e.kind === "ask_finished")
        .map((e) => [e.kind, e.data.ask, e.actor.name]),
    ).toEqual([
      ["ask_started", 1, "adam"],
      ["ask_started", 2, "reviewer"],
      ["ask_finished", 2, "verifier"],
      ["ask_finished", 1, "reviewer"],
      ["ask_started", 3, "adam"],
      ["ask_finished", 3, "verifier"],
    ]);
    const later = await validated(
      await frames(await connect(base, threadId, { mode: "run" })),
      "ask-agent replay",
    );
    expect(asks(later).map((a) => [a.id, a.state])).toEqual(told.map((a) => [a.id, a.state]));
  });

  it("ask-hold: both asks run until Cancel; a client that joins is told about them, parents first, and they end canceled, deepest first, before the run does", async () => {
    const { threadId } = await startThread("ask-hold coordinate the review");
    for (let i = 0; i < 200; i++) {
      const t = (await (await fetch(`${base}/api/threads/${threadId}`)).json()) as Thread;
      if (t.lastSeq >= 5) break; // the search of the verifier is the 5th event: the script now waits
      await new Promise((r) => setTimeout(r, 10));
    }
    const ac = new AbortController();
    const live = frames(await connect(base, threadId, { lastEventId: 5, signal: ac.signal }), (f) =>
      isTerminal(f),
    );
    expect((await post(`/api/threads/${threadId}/cancel`)).status).toBe(202);
    const list = await validated(await live, "ask-hold cancel");
    ac.abort();
    // the preamble: the invocation, then the asks that run, outermost first, before the first snapshot
    const preamble = list.filter((f) => f.id === undefined).slice(0, 4);
    expect(preamble.map((f) => f.event.type)).toEqual([
      "RUN_STARTED",
      "SUBAGENT_STARTED",
      "SUBAGENT_STARTED",
      "SUBAGENT_STARTED",
    ]);
    const joined = subagents(list, "SUBAGENT_STARTED");
    expect(joined.slice(1, 3)).toEqual(["sub-ask-1", "sub-ask-2"]);
    // Cancel: the asks that ran end canceled, "the asking task ended", deepest first, then the invocation
    const ended = kinds(list)
      .filter((e) => e.type === "SUBAGENT_FINISHED")
      .map((e) => [e.subagentRunId, (e.result as Ev | undefined)?.status]);
    expect(ended.slice(0, 2)).toEqual([
      ["sub-ask-2", "canceled"],
      ["sub-ask-1", "canceled"],
    ]);
    expect(ended).toHaveLength(3);
    const told = asks(list).filter((a) => a.state !== "running");
    expect(told.map((a) => [a.id, a.state, a.error])).toEqual([
      ["ask-2", "canceled", "the asking task ended"],
      ["ask-1", "canceled", "the asking task ended"],
    ]);
    expect(list.at(-1)?.event).toMatchObject({
      type: "RUN_FINISHED",
      outcome: { type: "cancelled" },
    });
    await waitForState(threadId, ["cancelled"]);
  });

  it("ask-hold: released, the asks end as in ask-agent, the second fails and the run is done", async () => {
    const { threadId } = await startThread("ask-hold coordinate the review");
    for (let i = 0; i < 200; i++) {
      const t = (await (await fetch(`${base}/api/threads/${threadId}`)).json()) as Thread;
      if (t.lastSeq >= 5) break;
      await new Promise((r) => setTimeout(r, 10));
    }
    expect((await post(`/__mock/release?thread=${threadId}`)).status).toBe(204);
    expect((await waitForState(threadId, ["done"])).state).toBe("done");
    const list = await validated(
      await frames(await connect(base, threadId, { mode: "run" })),
      "ask-hold released",
    );
    expect(asks(list).map((a) => [a.id, a.state])).toEqual([
      ["ask-1", "running"],
      ["ask-2", "running"],
      ["ask-2", "completed"],
      ["ask-1", "completed"],
      ["ask-3", "running"],
      ["ask-3", "failed"],
    ]);
  });
});

describe("sharing a thread by a link (ADR 0040), as the mock does it", () => {
  type Me = components["schemas"]["Me"];
  type Link = components["schemas"]["ThreadShareLink"];
  type Shared = components["schemas"]["SharedThread"];
  type Problem = { title: string; status: number; detail?: string; code?: string };

  let sessions = 0;
  /** A session of its own, as `me`, under a deployment whose cap on sharing is `cap`. */
  async function as(
    me: "user" | "admin" | "read-only",
    cap: "disabled" | "internal" | "public" = "public",
    signedIn = true,
  ) {
    const session = `share-${++sessions}`;
    const query = `me=${me}&sharing=${cap}&signedIn=${signedIn}&session=${session}`;
    expect((await post(`/__mock/config?${query}`)).status).toBe(204);
    return { Cookie: `mock-registry=${session}` };
  }
  const call = (method: string, p: string, headers: Record<string, string>, body?: unknown) =>
    fetch(base + p, {
      method,
      headers: { "Content-Type": "application/json", ...headers },
      ...(body === undefined ? {} : { body: JSON.stringify(body) }),
    });
  async function threadOf(headers: Record<string, string>, text = "echo shared", agent = "adam") {
    const threadId = newId();
    const res = await fetch(`${base}/agui/agents/${agent}`, {
      method: "POST",
      headers: { "Content-Type": "application/json", Accept: "text/event-stream", ...headers },
      body: JSON.stringify({
        state: {},
        tools: [],
        context: [],
        forwardedProps: {},
        threadId,
        runId: "run-1",
        messages: [{ id: "m-1", role: "user", content: text }],
      }),
    });
    expect(res.status).toBe(200);
    await frames(res);
    return threadId;
  }
  const share = (id: string, headers: Record<string, string>, visibility: unknown) =>
    call("PUT", `/api/threads/${id}/share`, headers, { visibility });
  const link = async (res: Response, status = 200): Promise<Link> => {
    expect(res.status).toBe(status);
    return (await expectDocumented("/api/threads/{threadId}/share", "put", res)) as Link;
  };
  const tokenOf = (l: Link) => (l.url ?? "").replace("/s/", "");
  const problemOf = async (
    template: string,
    method: string,
    res: Response,
    status: number,
    code?: string,
  ): Promise<Problem> => {
    expect(res.status).toBe(status);
    const body = (await expectDocumented(template, method, res)) as Problem;
    expect(body.code).toBe(code);
    return body;
  };
  const NOT_FOUND = "/api/shared/{token}";

  it("getMe says what the person may share as: the cap for a role that holds thread.share, else disabled", async () => {
    for (const [me, cap, said] of [
      ["user", "internal", "internal"],
      ["user", "public", "public"],
      ["user", "disabled", "disabled"],
      ["admin", "public", "public"],
      ["read-only", "public", "disabled"],
    ] as const) {
      const res = await call("GET", "/api/me", await as(me, cap));
      expect(((await expectDocumented("/api/me", "get", res)) as Me).sharing).toBe(said);
    }
    expect((await post("/__mock/config?sharing=everyone&session=share-bad")).status).toBe(400);
  });

  it("shareThread: a first share makes a link, a widening keeps it, the same level writes nothing", async () => {
    const owner = await as("user");
    const id = await threadOf(owner);
    const first = await link(await share(id, owner, "internal"));
    expect(first).toMatchObject({ visibility: "internal", effective: "internal" });
    expect(first.url).toMatch(/^\/s\/[A-Za-z0-9_-]{43}$/);

    const again = await link(await share(id, owner, "internal"));
    expect(again).toEqual(first);
    const wider = await link(await share(id, owner, "public"));
    expect(wider).toMatchObject({ visibility: "public", effective: "public" });
    expect(wider.url).toBe(first.url);

    // the thread says so: the link only to its owner, in `getThread`
    const one = await call("GET", `/api/threads/${id}`, owner);
    const thread = (await expectDocumented("/api/threads/{threadId}", "get", one)) as Thread;
    expect(thread.share).toEqual({ visibility: "public", effective: "public", url: first.url });
    // and the list marks it without the link
    const list = await call("GET", "/api/threads", owner);
    const items = (await expectDocumented("/api/threads", "get", list)) as Thread[];
    expect(items.find((t) => t.id === id)?.share).toEqual({
      visibility: "public",
      effective: "public",
    });
    // the log has the events, and the export carries no link
    const exported = await call("GET", `/api/threads/${id}/export`, owner);
    const document = (await exported.json()) as {
      thread: Thread;
      events: { kind: string; data: Record<string, unknown> }[];
    };
    expect(document.thread.share).toBeUndefined();
    const shared = document.events.filter((e) => e.kind === "thread_shared");
    expect(shared).toHaveLength(2);
    expect(JSON.stringify(document)).not.toContain(tokenOf(first));
  });

  it("shareThread: what it refuses, and why", async () => {
    const owner = await as("user", "internal");
    const id = await threadOf(owner);
    const T = "/api/threads/{threadId}/share";
    // `private` is not a value, nor is anything else, nor an extra member
    for (const body of [
      { visibility: "private" },
      { visibility: "world" },
      {},
      { visibility: "internal", x: 1 },
    ]) {
      await problemOf(T, "put", await call("PUT", `/api/threads/${id}/share`, owner, body), 400);
    }
    // above the cap
    await problemOf(T, "put", await share(id, owner, "public"), 409, "over_cap");
    // a deployment that has turned sharing off
    const off = await as("user", "disabled");
    const offId = await threadOf(off);
    await problemOf(T, "put", await share(offId, off, "internal"), 403, "sharing_disabled");
    // a role without thread.share
    const viewer = await as("read-only");
    await problemOf(T, "put", await share(id, viewer, "internal"), 403, "forbidden");
    // somebody else's thread is a 404 for every role, an administrator's included
    const other = await as("admin");
    await problemOf(T, "put", await share(id, other, "internal"), 404);
  });

  it("rotateThreadShare: a new link, and the old one is a 404; a private thread has none to replace", async () => {
    const owner = await as("user");
    const id = await threadOf(owner);
    const R = "/api/threads/{threadId}/share/rotate";
    await problemOf(
      R,
      "post",
      await call("POST", `/api/threads/${id}/share/rotate`, owner),
      409,
      "not_shared",
    );
    const first = await link(await share(id, owner, "internal"));
    const res = await call("POST", `/api/threads/${id}/share/rotate`, owner);
    const next = (await expectDocumented(R, "post", res)) as Link;
    expect(next.visibility).toBe("internal");
    expect(next.url).not.toBe(first.url);
    const reader = await as("admin");
    await problemOf(
      NOT_FOUND,
      "get",
      await call("GET", `/api/shared/${tokenOf(first)}`, reader),
      404,
    );
    const read = await call("GET", `/api/shared/${tokenOf(next)}`, reader);
    expect(read.status).toBe(200);
  });

  it("unshareThread: needs only ownership, is 204 whether or not it was shared, and the link is a 404", async () => {
    const owner = await as("user");
    const id = await threadOf(owner);
    const made = await link(await share(id, owner, "public"));
    // another person's DELETE is the 404 of a thread that is not theirs
    const other = await as("admin");
    await problemOf(
      "/api/threads/{threadId}/share",
      "delete",
      await call("DELETE", `/api/threads/${id}/share`, other),
      404,
    );
    // the owner takes it down even where the deployment has turned sharing off (the same person, a cap of disabled)
    const off = await as("user", "disabled");
    expect((await call("DELETE", `/api/threads/${id}/share`, off)).status).toBe(204);
    expect((await call("DELETE", `/api/threads/${id}/share`, off)).status).toBe(204);
    await problemOf(
      NOT_FOUND,
      "get",
      await call("GET", `/api/shared/${tokenOf(made)}`, await as("admin")),
      404,
    );
    const one = await call("GET", `/api/threads/${id}`, owner);
    expect(((await one.json()) as Thread).share).toBeUndefined();
    const log = (await (await call("GET", `/api/threads/${id}/export`, owner)).json()) as {
      events: { kind: string }[];
    };
    expect(log.events.filter((e) => e.kind === "thread_unshared")).toHaveLength(1);
  });

  it("getSharedThread: the reader's projection, with no owner; the owner is told so; every failure is one 404", async () => {
    const owner = await as("user");
    const id = await threadOf(owner, "echo what I pasted");
    const made = await link(await share(id, owner, "internal"));
    const token = tokenOf(made);

    const reader = await as("admin");
    const res = await call("GET", `/api/shared/${token}`, reader);
    expect(res.headers.get("cache-control")).toBe("no-store");
    expect(res.headers.get("x-robots-tag")).toBe("noindex, nofollow");
    const view = (await expectDocumented("/api/shared/{token}", "get", res)) as Shared;
    expect(view).toMatchObject({ id, visibility: "internal", isOwner: false, state: "done" });
    expect(JSON.stringify(view)).not.toContain("dev@example.com");
    expect(view).not.toHaveProperty("owner");
    expect(view).not.toHaveProperty("share");
    const mine = await call("GET", `/api/shared/${token}`, owner);
    expect(((await mine.json()) as Shared).isOwner).toBe(true);

    // a token that is wrong in any way, one nobody holds, and a role without thread.read: the same body
    const bodies = new Set<string>();
    for (const bad of [
      "short",
      `${token.slice(0, 42)}${token.endsWith("A") ? "B" : "A"}`,
      "A".repeat(43),
      `${token}x`,
    ]) {
      const miss = await call("GET", `/api/shared/${bad}`, reader);
      expect(miss.status).toBe(404);
      bodies.add(JSON.stringify(await miss.json()));
    }
    expect(bodies.size).toBe(1);
    // an internal link is not for anybody, and a public one is for both
    const out = await as("user", "public", false);
    await problemOf(
      "/api/public/shared/{token}",
      "get",
      await call("GET", `/api/public/shared/${token}`, out),
      404,
    );
    await link(await share(id, owner, "public"));
    const open = await call("GET", `/api/public/shared/${token}`, out);
    expect(open.headers.get("x-robots-tag")).toBe("noindex, nofollow");
    const anybody = (await expectDocumented("/api/public/shared/{token}", "get", open)) as Shared;
    expect(anybody).toMatchObject({ visibility: "public", isOwner: false });
    // not signed in: the signed-in route is the 401 that sends a browser to the public one
    await problemOf(
      "/api/shared/{token}",
      "get",
      await call("GET", `/api/shared/${token}`, out),
      401,
    );
    // a cap that was lowered pauses it, and `getThread` says so
    const lowered = await as("user", "internal");
    const paused = await call("GET", `/api/threads/${id}`, lowered);
    expect(((await paused.json()) as Thread).share).toMatchObject({
      visibility: "public",
      effective: "internal",
    });
    await problemOf(
      "/api/public/shared/{token}",
      "get",
      await call("GET", `/api/public/shared/${token}`, await as("user", "internal", false)),
      404,
    );
  });

  it("the stream of a reader: the log without the owner's address, ended when the link goes", async () => {
    const owner = await as("user");
    const id = await threadOf(owner, "echo hello there");
    const made = await link(await share(id, owner, "public"));
    const token = tokenOf(made);
    const reader = await as("admin");

    for (const route of [`/agui/shared/${token}/connect`, `/agui/public/shared/${token}/connect`]) {
      const res = await fetch(`${base}${route}?mode=run`, {
        headers: { Accept: "text/event-stream", ...reader },
      });
      expect(res.status).toBe(200);
      const list = await validated(await frames(res), route);
      const text = JSON.stringify(list);
      expect(text).not.toContain("dev@example.com");
      expect(text).toContain("the owner");
      expect(list.at(-1)?.event.type).toBe("RUN_FINISHED");
      // the numbering of the log holds: the sharing events say nothing, so the last id is the run's
      expect(Math.max(...list.flatMap((f) => (f.id === undefined ? [] : [f.id])))).toBeGreaterThan(
        0,
      );
    }

    // a stream that is open when the link is replaced ends, and its reconnect is the 404
    const open = await fetch(`${base}/agui/shared/${token}/connect`, {
      headers: { Accept: "text/event-stream", ...reader },
    });
    expect(open.status).toBe(200);
    const done = frames(open);
    await new Promise((r) => setTimeout(r, 50));
    const next = (await expectDocumented(
      "/api/threads/{threadId}/share/rotate",
      "post",
      await call("POST", `/api/threads/${id}/share/rotate`, owner),
    )) as Link;
    await done; // ends by itself
    const again = await fetch(`${base}/agui/shared/${token}/connect`, {
      headers: { Accept: "text/event-stream", ...reader },
    });
    await problemOf("/agui/shared/{token}/connect", "get", again, 404);
    expect(tokenOf(next)).not.toBe(token);
  });

  it("the files of a shared thread: by the shared route for a signed-in reader, and not for the public", async () => {
    const owner = await as("user");
    const id = await threadOf(owner, "files make some", "reviewer");
    const made = await link(await share(id, owner, "public"));
    const token = tokenOf(made);
    const log = (await (await call("GET", `/api/threads/${id}/export`, owner)).json()) as {
      events: { kind: string; data: { file?: { sha256: string } } }[];
    };
    const sha = log.events.find((e) => e.data.file)?.data.file?.sha256 as string;
    expect(sha).toMatch(/^[0-9a-f]{64}$/);

    const reader = await as("admin");
    const file = await call("GET", `/api/shared/${token}/artifacts/${sha}`, reader);
    expect(file.status).toBe(200);
    expect(file.headers.get("cache-control")).toBe("no-store");
    expect(file.headers.get("x-content-type-options")).toBe("nosniff");
    // one the log does not name is the 404
    const other = "0".repeat(64);
    await problemOf(
      "/api/shared/{token}/artifacts/{sha256}",
      "get",
      await call("GET", `/api/shared/${token}/artifacts/${other}`, reader),
      404,
    );
    // anybody has no files unless the deployment says so, and the stream does not name them
    const out = await as("user", "public", false);
    await problemOf(
      "/api/public/shared/{token}/artifacts/{sha256}",
      "get",
      await call("GET", `/api/public/shared/${token}/artifacts/${sha}`, out),
      404,
    );
    const stream = await fetch(`${base}/agui/public/shared/${token}/connect?mode=run`, {
      headers: { Accept: "text/event-stream", ...out },
    });
    expect(JSON.stringify(await frames(stream))).not.toContain(sha);
    const signed = await fetch(`${base}/agui/shared/${token}/connect?mode=run`, {
      headers: { Accept: "text/event-stream", ...reader },
    });
    expect(JSON.stringify(await frames(signed))).toContain(sha);
  });
});

describe("the stand-in for the edge's own routes (web/README.md, Signing in again)", () => {
  let sessions = 0;
  const as = async (query = "") => {
    const session = `edge-${++sessions}`;
    expect((await post(`/__mock/config?session=${session}&${query}`)).status).toBe(204);
    return { session, headers: { Cookie: `mock-registry=${session}` } };
  };
  const edge = async (session: string) =>
    (await fetch(`${base}/__mock/edge?session=${session}`)).json() as Promise<{
      signedIn: boolean;
      stale: boolean;
      refreshes: number;
      signIns: number;
    }>;

  it("a session is a 200 on userinfo and nothing else changes", async () => {
    const { session, headers } = await as();
    const res = await fetch(`${base}/oauth2/userinfo`, { headers });
    expect(res.status).toBe(200);
    expect(await res.json()).toMatchObject({ user: expect.any(String) });
    expect(await edge(session)).toMatchObject({ stale: false, refreshes: 0 });
  });

  it("no session is a plain 401 on userinfo, and on every route that is not public", async () => {
    const { headers } = await as("signedIn=false");
    const userinfo = await fetch(`${base}/oauth2/userinfo`, { headers });
    expect(userinfo.status).toBe(401);
    expect(await userinfo.text()).toContain("Unauthorized");
    expect((await fetch(`${base}/api/me`, { headers })).status).toBe(401);
  });

  it("a stale token is a 401 on the API until userinfo refreshes it, once", async () => {
    const { session, headers } = await as("stale=true");
    expect((await fetch(`${base}/api/me`, { headers })).status).toBe(401);
    expect((await fetch(`${base}/agui/agents/adam`, { method: "POST", headers })).status).toBe(401);
    expect((await fetch(`${base}/oauth2/userinfo`, { headers })).status).toBe(200);
    expect((await fetch(`${base}/api/me`, { headers })).status).toBe(200);
    // a second question has nothing to refresh
    expect((await fetch(`${base}/oauth2/userinfo`, { headers })).status).toBe(200);
    expect(await edge(session)).toMatchObject({ stale: false, refreshes: 1 });
  });

  it("start signs the session in and sends the browser to a path of this origin, never anywhere else", async () => {
    const { session, headers } = await as("signedIn=false");
    const back = await fetch(`${base}/oauth2/start?rd=/signed-in`, { headers, redirect: "manual" });
    expect(back.status).toBe(302);
    expect(back.headers.get("location")).toBe("/signed-in");
    expect((await fetch(`${base}/api/me`, { headers })).status).toBe(200);
    expect(await edge(session)).toMatchObject({ signedIn: true, signIns: 1 });
    for (const rd of ["//evil.example/x", "https://evil.example/x"]) {
      const res = await fetch(`${base}/oauth2/start?rd=${encodeURIComponent(rd)}`, {
        headers,
        redirect: "manual",
      });
      expect(res.headers.get("location")).toBe("/");
    }
  });

  it("the public routes are outside it: a stale or signed-out session still reads a public link", async () => {
    const { headers } = await as("signedIn=false&stale=true");
    // not a link that exists, but the answer is the route's own (404), never the identity layer's 401
    const res = await fetch(`${base}/api/public/shared/${"x".repeat(43)}`, { headers });
    expect(res.status).toBe(404);
  });
});
