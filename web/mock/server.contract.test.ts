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
  agent = "coder",
  extra: { forwardedProps?: Record<string, unknown> } = {},
) {
  // A run that waits (slow, verify-wait, verify-reviewed-wait) never ends its response: read up to RUN_STARTED.
  const untilStarted =
    text.startsWith("slow") ||
    text.startsWith("verify-wait") ||
    text.startsWith("verify-reviewed-wait");
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
  it("lists agents; only coder advertises releases", async () => {
    const res = await fetch(`${base}/api/agents`);
    const agents = (await expectDocumented(
      "/api/agents",
      "get",
      res,
    )) as components["schemas"]["Agent"][];
    expect(agents.find((a) => a.id === "coder")?.releases?.defaultChannel).toBe("production");
    expect(agents.find((a) => a.id === "reviewer")?.releases).toBeUndefined();
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
      expect(agents.map((a) => a.id)).toEqual(["coder", "reviewer", "verifier", "helper"]);
      expect(agents[3]).toMatchObject({ source: "registry", tags: ["writing"] });

      // down: none of the registry's agents, and the status says which source and why
      await post("/__mock/registry?down=true");
      expect((await list()).map((a) => a.id)).toEqual(["coder", "reviewer", "verifier"]);
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
    expect((await list()).map((a) => a.id)).toEqual(["coder", "reviewer", "verifier"]);
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
      target: { agentId: "coder" },
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
    const answer = await postRun(base, "coder", {
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
    const { threadId, body } = await startThread("Use staging", "coder", {
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
      [400, "coder", { ...good, threadId: "not-a-uuid" }],
      [404, "nope", good],
      [
        400,
        "reviewer",
        { ...good, forwardedProps: { [RELEASE_CHANNELS_URI]: { release: "staging" } } },
      ],
      [400, "coder", { ...good, forwardedProps: { [RELEASE_CHANNELS_URI]: { release: "nope" } } }],
      [422, "coder", { ...good, messages: [] }],
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
    const other = await postRun(base, "coder", {
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
      ["coder", true],
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
    const { body } = await startThread("Implement the thing", "coder", {
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
      const res = await postRun(base, "coder", {
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
    const { threadId } = await startThread("ask a question", "coder", {
      forwardedProps: { [UI_CATALOG_PROP]: v1 },
    });
    await waitForState(threadId, ["blocked"]);
    // the answer carries v2: the snapshots of that run say 2
    const answer = await postRun(base, "coder", {
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
    const next = await postRun(base, "coder", {
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
    postRun(at, "coder", {
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
  async function twoJobs(agent = "coder") {
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
    expect(forked.target).toEqual({ agentId: "coder" });
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
      target: { agentId: "coder" },
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
    const refused = await postRun(base, "coder", {
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
      ["an unknown release", { after: 1, target: { agentId: "coder", release: "nope" } }, 400],
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
    expect(done.target).toEqual({ agentId: "coder" });
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
