import { readFileSync } from "node:fs";
import type { AddressInfo } from "node:net";
import path from "node:path";
import Ajv2020 from "ajv/dist/2020.js";
import addFormats from "ajv-formats";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { parse } from "yaml";
import type { components } from "../src/api/schema";
import { createMockServer } from "./server";

/**
 * Keeps the mock honest: every response it produces (JSON bodies and SSE frames) is validated
 * against the schemas in docs/api/chat-api.yaml.
 */

type Thread = components["schemas"]["Thread"];
type Event = components["schemas"]["Event"];

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
ajv.addSchema({ $id: "chat", components: contract.components });

const rewriteRefs = (schema: unknown) =>
  JSON.parse(JSON.stringify(schema).replaceAll('"#/components', '"chat#/components'));

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

async function create(text: string, target: Thread["target"] = { agentId: "coder" }) {
  const res = await post("/api/threads", { target, text });
  const body = (await expectDocumented("/api/threads", "post", res)) as Thread;
  expect(res.status).toBe(201);
  return body;
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

const getEvents = async (id: string, query = "") => {
  const res = await fetch(`${base}/api/threads/${id}/events${query}`);
  return (await expectDocumented("/api/threads/{threadId}/events", "get", res)) as Event[];
};

/** Read the SSE stream until `count` data frames arrived, validating framing and payloads. */
async function readStream(id: string, count: number, lastEventId?: number) {
  const ac = new AbortController();
  const res = await fetch(`${base}/api/threads/${id}/stream`, {
    signal: ac.signal,
    headers: lastEventId === undefined ? {} : { "Last-Event-ID": String(lastEventId) },
  });
  await expectDocumented("/api/threads/{threadId}/stream", "get", res);
  expect(res.headers.get("content-type")).toContain("text/event-stream");
  const frames: { id: string; event: string; data: Event }[] = [];
  const decoder = new TextDecoder();
  let buf = "";
  const reader = res.body?.getReader();
  if (!reader) throw new Error("no body");
  while (frames.length < count) {
    const { value, done } = await reader.read();
    if (done) break;
    buf += decoder.decode(value, { stream: true });
    let i = buf.indexOf("\n\n");
    while (i >= 0) {
      const raw = buf.slice(0, i);
      buf = buf.slice(i + 2);
      i = buf.indexOf("\n\n");
      const fields = Object.fromEntries(
        raw
          .split("\n")
          .filter((l) => l && !l.startsWith(":") && l.includes(":"))
          .map((l) => [l.slice(0, l.indexOf(":")), l.slice(l.indexOf(":") + 1).trim()]),
      );
      if (!fields.data) continue; // retry: / keepalive
      const data = JSON.parse(fields.data) as Event;
      validateAgainst({ $ref: "#/components/schemas/Event" }, data, "SSE data frame");
      frames.push({ id: fields.id ?? "", event: fields.event ?? "", data });
    }
  }
  ac.abort();
  return frames;
}

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

  it("happy path: 201, ordered events, SSE ids/kinds, Last-Event-ID replay", async () => {
    const t = await create("Implement the thing");
    expect(t).toMatchObject({ state: "queued", lastSeq: 1, target: { agentId: "coder" } });
    const done = await waitForState(t.id, ["done"]);
    const evts = await getEvents(t.id);
    expect(evts.map((e) => e.seq)).toEqual(evts.map((_, i) => i + 1));
    expect(done.lastSeq).toBe(evts.length);
    expect(evts.map((e) => e.kind)).toEqual([
      "user_message",
      "agent_status",
      "agent_message",
      "agent_message",
      "artifact",
      "agent_status",
      "thread_state",
    ]);
    const [partial, final] = evts.filter((e) => e.kind === "agent_message");
    expect(partial?.data.messageId).toBe(final?.data.messageId);
    expect([partial?.data.final, final?.data.final]).toEqual([false, true]);
    expect(evts[1]?.actor).toEqual({ type: "agent", name: "coder", revision: "coder-r47" });

    const all = await readStream(t.id, evts.length);
    expect(all.map((f) => Number(f.id))).toEqual(evts.map((e) => e.seq));
    expect(all.every((f) => f.event === f.data.kind && Number(f.id) === f.data.seq)).toBe(true);

    const replay = await readStream(t.id, evts.length - 3, 3);
    expect(replay.map((f) => f.data.seq)).toEqual(evts.slice(3).map((e) => e.seq));
    expect(await getEvents(t.id, "?after=5")).toHaveLength(evts.length - 5);
  });

  it("streams live events after the replay", async () => {
    const t = await create("Another one");
    const frames = await readStream(t.id, 7);
    expect(frames.map((f) => f.data.seq)).toEqual([1, 2, 3, 4, 5, 6, 7]);
  });

  it("uses the selected release for the actor revision", async () => {
    const t = await create("Use staging", { agentId: "coder", release: "staging" });
    expect(t.target.release).toBe("staging");
    const evts = await getEvents(t.id);
    await waitForState(t.id, ["done"]);
    expect((await getEvents(t.id)).some((e) => e.actor.revision === "coder-r51")).toBe(true);
    expect(evts[0]?.kind).toBe("user_message");
  });

  it("rejects bad thread creation with problem+json 400", async () => {
    for (const body of [
      { target: { agentId: "nope" }, text: "hi" },
      { target: { agentId: "reviewer", release: "staging" }, text: "hi" },
      { target: { agentId: "coder", release: "nope" }, text: "hi" },
      { target: { agentId: "coder" }, text: "" },
    ]) {
      const res = await post("/api/threads", body);
      expect(res.status).toBe(400);
      await expectDocumented("/api/threads", "post", res);
    }
  });

  it("answers 404 problems for unknown threads", async () => {
    const id = "00000000-0000-4000-8000-000000000000";
    const cases: [string, string, string][] = [
      ["/api/threads/{threadId}", "get", `/api/threads/${id}`],
      ["/api/threads/{threadId}/events", "get", `/api/threads/${id}/events`],
      ["/api/threads/{threadId}/stream", "get", `/api/threads/${id}/stream`],
    ];
    for (const [tpl, method, p] of cases) {
      const res = await fetch(base + p);
      expect(res.status).toBe(404);
      await expectDocumented(tpl, method, res);
    }
    for (const [tpl, p] of [
      ["/api/threads/{threadId}/messages", `/api/threads/${id}/messages`],
      ["/api/threads/{threadId}/cancel", `/api/threads/${id}/cancel`],
    ] as const) {
      const res = await post(p, { text: "x" });
      expect(res.status).toBe(404);
      await expectDocumented(tpl, "post", res);
    }
  });

  it("blocked thread: an answer resumes it; a finished thread answers 409", async () => {
    const t = await create("A question for you");
    const blocked = await waitForState(t.id, ["blocked"]);
    const evts = await getEvents(t.id);
    expect(evts.at(-1)).toMatchObject({ kind: "thread_state", data: { state: "blocked" } });
    expect(evts.find((e) => e.data.status === "input_required")?.data.detail).toMatch(/branch/);
    expect(blocked.lastSeq).toBe(evts.length);

    const res = await post(`/api/threads/${t.id}/messages`, { text: "main" });
    expect(res.status).toBe(202);
    const echoed = (await expectDocumented(
      "/api/threads/{threadId}/messages",
      "post",
      res,
    )) as Event;
    expect(echoed).toMatchObject({
      kind: "user_message",
      data: { text: "main" },
      actor: { type: "user" },
    });
    await waitForState(t.id, ["done"]);

    const late = await post(`/api/threads/${t.id}/messages`, { text: "more" });
    expect(late.status).toBe(409);
    await expectDocumented("/api/threads/{threadId}/messages", "post", late);
  });

  it("cancel: 202, then agent_status canceled and thread_state cancelled", async () => {
    const t = await create("a slow task");
    await waitForState(t.id, ["working"]);
    const res = await post(`/api/threads/${t.id}/cancel`);
    expect(res.status).toBe(202);
    await expectDocumented("/api/threads/{threadId}/cancel", "post", res);
    await waitForState(t.id, ["cancelled"]);
    const evts = await getEvents(t.id);
    expect(evts.map((e) => e.kind).slice(-2)).toEqual(["agent_status", "thread_state"]);
    expect(evts.at(-2)?.data.status).toBe("canceled");
  });

  it("failure: error event, failed status, thread_state failed", async () => {
    const t = await create("please fail");
    await waitForState(t.id, ["failed"]);
    const evts = await getEvents(t.id);
    expect(evts.find((e) => e.kind === "error")?.data).toEqual({
      message: "Agent crashed",
      retryable: false,
    });
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
