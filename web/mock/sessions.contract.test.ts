import { readFileSync } from "node:fs";
import path from "node:path";
import Ajv2020 from "ajv/dist/2020.js";
import addFormats from "ajv-formats";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { parse } from "yaml";
import { parseSession, parseSessionStart } from "../src/features/sessions/lib/cards";
import { connect, type Frame, frames } from "./agui-client";
import { CHILD_SESSIONS_URI } from "./sessions";
import { calls, type Mock, newId, newRequest, run, startMock } from "./sessions-client";

/**
 * Keeps the mock's child sessions honest against docs/api/chat-api.yaml (ADR 0072): every answer of
 * `startChildSession` and `closeChildSession`, of a run on a child thread, of a fork or a share of one,
 * and of a thread that is one, every event it writes, and every frame of its streams, against the
 * contract; and every status the contract documents for the two new routes is one the mock answers.
 * The cards have no schema in the contract, so their schemas are written here from docs/api/agui.md.
 */

const contract = parse(
  readFileSync(path.resolve(import.meta.dirname, "../../docs/api/chat-api.yaml"), "utf8"),
) as {
  paths: Record<string, Record<string, { responses: Record<string, unknown> }>>;
  components: Record<string, unknown>;
};

const interop = <T>(m: T): T => (m as unknown as { default?: T }).default ?? m;
const ajv = new (interop(Ajv2020))({ strict: false, allErrors: true });
interop(addFormats)(ajv);
ajv.addFormat("int64", true);
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
  expect(
    validate(body),
    `${label}: ${JSON.stringify(validate.errors)} in ${JSON.stringify(body)}`,
  ).toBe(true);
}

/** The statuses each operation was answered with, to hold against what the contract documents. */
const seen = new Map<string, Set<string>>();

/** `res` is a documented response of `method template` and its JSON body matches the schema. */
async function documented(template: string, method: string, res: Response): Promise<unknown> {
  const op = contract.paths[template]?.[method];
  expect(op, `${method} ${template} is in the contract`).toBeDefined();
  const key = `${method} ${template}`;
  seen.set(key, (seen.get(key) ?? new Set()).add(String(res.status)));
  const def = op?.responses[String(res.status)];
  expect(def, `${key} documents ${res.status}`).toBeDefined();
  const resolved = resolveRef<{ content?: Record<string, { schema: unknown }> }>(def);
  if (!resolved.content) return undefined;
  const contentType = (res.headers.get("content-type") ?? "").split(";")[0] ?? "";
  const media = resolved.content[contentType];
  expect(media, `content-type ${contentType} is documented`).toBeDefined();
  if (contentType === "text/event-stream") return undefined;
  const body: unknown = await res.json();
  validateAgainst(media?.schema, body, `${key} ${res.status}`);
  return body;
}

// The cards of docs/api/agui.md, as JSON Schema: every member, and no other.
const STATE = { enum: ["working", "idle", "closed"] };
const AT = { type: "string" };
const CARDS = {
  "vymalo.session": {
    parent: {
      type: "object",
      additionalProperties: false,
      required: ["session", "threadId", "agent", "description", "keepOpen", "state", "at"],
      properties: {
        session: { type: "string", minLength: 1 },
        threadId: { type: "string", format: "uuid" },
        agent: { type: "string" },
        description: { type: "string" },
        keepOpen: { type: "boolean" },
        state: STATE,
        by: { const: "person" },
        round: { type: "integer", minimum: 1 },
        report: { enum: ["done", "failed", "canceled", "blocked"] },
        at: AT,
      },
    },
    self: {
      type: "object",
      additionalProperties: false,
      required: ["session", "agent", "keepOpen", "state", "closing", "closeAsked", "at"],
      properties: {
        session: { type: "string", minLength: 1 },
        agent: { type: "string" },
        keepOpen: { type: "boolean" },
        state: STATE,
        closing: { type: "boolean" },
        closeAsked: { type: "boolean" },
        at: AT,
      },
    },
  },
  "vymalo.sessionStart": {
    type: "object",
    additionalProperties: false,
    required: ["request", "agent", "description", "keepOpen", "state", "at"],
    properties: {
      request: { type: "string", format: "uuid" },
      agent: { type: "string" },
      description: { type: "string" },
      keepOpen: { type: "boolean" },
      state: { enum: ["starting", "opened", "failed"] },
      threadId: { type: "string", format: "uuid" },
      session: { type: "string" },
      reason: { type: "string" },
      at: AT,
    },
  },
} as const;

/** Every frame of a stream is an AG-UI event, and every session card has the members of its card. */
function validFrames(list: Frame[], label: string): Frame[] {
  const started = new Set<string>();
  for (const f of list) {
    validateAgainst({ $ref: "#/components/schemas/AgUiEvent" }, f.event, label);
    const e = f.event;
    if (e.type === "RUN_STARTED") started.clear();
    if (e.type === "SUBAGENT_STARTED" && String(e.subagentRunId).startsWith("sub-session-")) {
      expect(String(e.subagentRunId), `${label}: a session's subagent id`).toMatch(
        /^sub-session-\d+(-\d+)?$/,
      );
      expect(
        started.has(String(e.subagentRunId)),
        `${label}: ${e.subagentRunId} twice in a run`,
      ).toBe(false);
      started.add(String(e.subagentRunId));
      expect((e.metadata as Record<string, unknown>)["vymalo.childThread"]).toMatch(
        /^[0-9a-f-]{36}$/,
      );
      expect(typeof (e.metadata as Record<string, unknown>)["vymalo.session"]).toBe("string");
    }
    if (e.type === "ACTIVITY_SNAPSHOT" && e.activityType === "vymalo.session") {
      const self = e.messageId === "session-self";
      expect(String(e.messageId)).toMatch(/^(session-self|session-\d+)$/);
      validateAgainst(
        CARDS["vymalo.session"][self ? "self" : "parent"],
        e.content,
        `${label}: ${e.messageId}`,
      );
      expect(e.replace).toBe(true);
      // and the web's reader takes what the contract's schema takes
      expect(parseSession(e.content), `${label}: parseSession of ${e.messageId}`).not.toBeNull();
    }
    if (e.type === "ACTIVITY_SNAPSHOT" && e.activityType === "vymalo.sessionStart") {
      expect(String(e.messageId)).toMatch(/^session-start-[0-9a-f-]{36}$/);
      validateAgainst(CARDS["vymalo.sessionStart"], e.content, `${label}: ${e.messageId}`);
      expect(e.replace).toBe(true);
      expect(parseSessionStart(e.content), `${label}: parseSessionStart`).not.toBeNull();
    }
    if (e.type === "ACTIVITY_SNAPSHOT" && e.activityType === "vymalo.job" && e.content) {
      const round = (e.content as { round?: unknown }).round;
      if (round !== undefined) {
        validateAgainst(
          {
            type: "object",
            required: ["number"],
            additionalProperties: false,
            properties: {
              number: { type: "integer", minimum: 1 },
              by: { enum: ["agent", "person"] },
            },
          },
          round,
          `${label}: the round of a job`,
        );
      }
    }
  }
  return list;
}

let mock: Mock;
beforeAll(async () => {
  mock = await startMock();
});
afterAll(async () => {
  await mock.close();
});

async function person(config?: string) {
  const who = await mock.session(config);
  return { who, ...calls(mock, who) };
}

const START = "/api/threads/{threadId}/sessions";
const CLOSE = "/api/threads/{threadId}/session/close";

describe("the mock honours docs/api/chat-api.yaml for child sessions", () => {
  it("startChildSession: every answer is documented and its body matches", async () => {
    const p = await person();
    const parent = await p.answered();
    const request = newRequest();
    const accepted = await p.start(parent, { id: request });
    expect(accepted.headers.get("Location")).toBe(`/api/threads/${parent}`);
    expect(await documented(START, "post", accepted)).toEqual({ request });
    // a repeat
    expect(await documented(START, "post", await p.start(parent, { id: request }))).toEqual({
      request,
    });
    await p.until(
      async () => (await p.of(parent, "child_session_start_ended")).length === 1,
      "the outcome",
    );

    const body = { id: newRequest(), agent: "planner", description: "x", text: "y" };
    const post = (b: unknown, id = parent) => p.json("POST", `/api/threads/${id}/sessions`, b);
    expect(
      ((await documented(START, "post", await post("{nope"))) as { status: number }).status,
    ).toBe(400);
    await documented(START, "post", await post({ ...body, extra: true }));
    await documented(START, "post", await post({ ...body, agent: "nobody" }));
    await documented(START, "post", await post({ ...body, description: "" }));
    await documented(START, "post", await post({ ...body, text: "" }));
    await documented(START, "post", await post(body, newId()));
    await documented(START, "post", await (await person()).start(parent));
    await documented(START, "post", await (await person("me=read-only")).start(parent));

    // no_child_sessions
    await p.config("sessions=off");
    await documented(START, "post", await p.start(parent));
    await p.config("sessions=on");

    // child_thread
    const { child } = await p.opened(parent, { keepOpen: true });
    await documented(START, "post", await p.start(child));

    // too_many_sessions (the thread notes two already)
    await p.config("sessionsNoted=2");
    await documented(START, "post", await p.start(parent));
    await p.config("sessionsNoted=256");

    // too_many_starts
    const holder = await person("sessionStarts=hold");
    const held = await holder.answered();
    for (let i = 0; i < 4; i++) await holder.start(held);
    await documented(START, "post", await holder.start(held));

    // no_context
    const slow = await startMock(60_000);
    try {
      const who = await slow.session();
      const id = newId();
      await run(slow, who, "adam", id, "echo hello");
      await documented(START, "post", await calls(slow, who).start(id));
    } finally {
      await slow.close();
    }

    // 503: the registry cannot be read, and 401: nobody is signed in
    const down = await person();
    const downParent = await down.answered();
    await down.config("sessions=unreadable");
    const unavailable = await down.start(downParent);
    expect(unavailable.headers.get("Retry-After")).not.toBeNull();
    await documented(START, "post", unavailable);
    await down.config("sessions=on&signedIn=false");
    await documented(START, "post", await down.start(downParent));
  });

  it("closeChildSession: every answer is documented and its body matches", async () => {
    const p = await person();
    const parent = await p.answered();
    const waiting = await p.opened(parent, { keepOpen: true, text: "hold on" });
    await p.reaches(waiting.child, ["working"]);
    expect(await documented(CLOSE, "post", await p.close(waiting.child))).toEqual({
      state: "closing",
    });
    await p.release(waiting.child);
    await p.until(
      async () => (await p.thread(waiting.child)).childOf?.state === "closed",
      "closed",
    );
    expect(await documented(CLOSE, "post", await p.close(waiting.child))).toEqual({
      state: "closed",
    });
    // an idle one closes at once, with no body at all
    const idle = await p.opened(parent, { keepOpen: true });
    await p.until(async () => (await p.thread(idle.child)).childOf?.state === "idle", "idle");
    expect(await documented(CLOSE, "post", await p.close(idle.child))).toEqual({ state: "closed" });
    // refused
    await documented(CLOSE, "post", await p.close(idle.child, '{"cancel":1}'));
    await documented(CLOSE, "post", await p.close(newId()));
    await documented(CLOSE, "post", await (await person()).close(idle.child));
    await documented(CLOSE, "post", await (await person("me=read-only")).close(idle.child));
    await documented(CLOSE, "post", await p.close(parent));
    const open = await p.opened(parent, { keepOpen: true });
    await p.until(async () => (await p.thread(open.child)).childOf?.state === "idle", "idle");
    await p.config("sessions=off");
    await documented(CLOSE, "post", await p.close(open.child));
    await p.config("sessions=on");
    // 503 and 401
    await p.config("sessions=unreadable");
    const unavailable = await p.close(open.child);
    expect(unavailable.headers.get("Retry-After")).not.toBeNull();
    await documented(CLOSE, "post", unavailable);
    await p.config("sessions=on&signedIn=false");
    await documented(CLOSE, "post", await p.close(open.child));
  });

  it("every status the contract documents for the two routes is one the mock answers", () => {
    for (const [template, method] of [
      [START, "post"],
      [CLOSE, "post"],
    ] as const) {
      const documentedStatuses = Object.keys(contract.paths[template]?.[method]?.responses ?? {});
      const answered = [...(seen.get(`${method} ${template}`) ?? [])];
      expect(answered.sort(), `${method} ${template}`).toEqual(documentedStatuses.sort());
    }
  });

  it("a run on a child thread: its refusals are documented problems", async () => {
    const p = await person();
    const parent = await p.answered();
    const kept = await p.opened(parent, { keepOpen: true });
    await p.until(async () => (await p.thread(kept.child)).childOf?.state === "idle", "idle");
    const mention = [{ agentId: "reviewer", label: "@reviewer", start: 0, end: 9 }];
    const mentioned = await run(mock, p.who, "adam", kept.child, "@reviewer hi", {
      "vymalo.mentions": mention,
    });
    expect(mentioned.status).toBe(422);
    await documented("/agui/agents/{agentId}", "post", mentioned);

    const once = await p.opened(parent, { keepOpen: false, text: "hold on" });
    await p.reaches(once.child, ["working"]);
    const stop = await run(mock, p.who, "adam", once.child, "again", {
      "vymalo.send": "interrupt",
    });
    expect(stop.status).toBe(422);
    await documented("/agui/agents/{agentId}", "post", stop);
    await p.release(once.child);
    await p.until(async () => (await p.thread(once.child)).childOf?.state === "closed", "closed");
    const closed = await run(mock, p.who, "adam", once.child, "hello");
    expect(closed.status).toBe(409);
    await documented("/agui/agents/{agentId}", "post", closed);
    // a stop of the round answers as it does for any thread
    const working = await p.opened(parent, { keepOpen: true, text: "hold on" });
    await p.reaches(working.child, ["working"]);
    await documented(
      "/api/threads/{threadId}/cancel",
      "post",
      await p.json("POST", `/api/threads/${working.child}/cancel`),
    );
  });

  it("a fork and a share of a child thread are documented refusals", async () => {
    const p = await person();
    const parent = await p.answered();
    const { child } = await p.opened(parent, { keepOpen: true });
    await documented(
      "/api/threads/{threadId}/fork",
      "post",
      await p.json("POST", `/api/threads/${child}/fork`, { after: 1 }),
    );
    await documented(
      "/api/threads/{threadId}/share",
      "put",
      await p.json("PUT", `/api/threads/${child}/share`, { visibility: "internal" }),
    );
  });

  it("a child thread, its list and its export match the contract, `childOf` and every event included", async () => {
    const p = await person();
    const id = newId();
    await (await run(mock, p.who, "adam", id, "helpers go")).text();
    await p.until(async () => (await p.of(id, "child_session_closed")).length === 2, "the closes");
    const started = await p.opened(id, { keepOpen: true });
    await p.close(started.child);
    const opened = await p.of(id, "child_session_opened");
    for (const e of opened) {
      const thread = await documented(
        "/api/threads/{threadId}",
        "get",
        await p.json("GET", `/api/threads/${e.data.thread}`),
      );
      expect((thread as { childOf?: unknown }).childOf).toBeDefined();
      const exported = await documented(
        "/api/threads/{threadId}/export",
        "get",
        await p.json("GET", `/api/threads/${e.data.thread}/export`),
      );
      expect((exported as { events: unknown[] }).events.length).toBeGreaterThan(2);
    }
    await documented("/api/threads", "get", await p.json("GET", "/api/threads?order=rail"));
    await documented("/api/threads", "get", await p.json("GET", "/api/threads"));
    const parentExport = await documented(
      "/api/threads/{threadId}/export",
      "get",
      await p.json("GET", `/api/threads/${id}/export`),
    );
    const kinds = new Set(
      (parentExport as { events: { kind: string }[] }).events.map((e) => e.kind),
    );
    for (const kind of [
      "child_session_opened",
      "child_session_reported",
      "child_session_closed",
      "child_session_start_asked",
      "child_session_start_ended",
    ]) {
      expect(kinds.has(kind), kind).toBe(true);
    }
    const childKinds = new Set<string>((await p.log(started.child)).map((e) => e.kind));
    for (const kind of ["child_session_close_asked", "child_session_state"]) {
      expect(childKinds.has(kind), kind).toBe(true);
    }
  });

  it("the capabilities document carries the offer", async () => {
    const p = await person();
    const body = (await documented(
      "/agui/agents/{agentId}/capabilities",
      "get",
      await p.json("GET", "/agui/agents/adam/capabilities"),
    )) as { custom?: Record<string, unknown> };
    expect(body.custom?.[CHILD_SESSIONS_URI]).toEqual({
      agents: [
        { name: "planner", description: "Breaks a request into steps." },
        { name: "researcher", description: "Looks things up on the web." },
      ],
      maxOpen: 4,
    });
  });
});

describe("the streams of child sessions are AG-UI and their cards are the contract's", () => {
  it("an agent's helpers: the parent's stream and each child's", async () => {
    const p = await person();
    const id = newId();
    await (await run(mock, p.who, "adam", id, "helpers go")).text();
    await p.until(async () => (await p.of(id, "child_session_closed")).length === 2, "the closes");
    const parent = validFrames(
      await frames(await connect(mock.base, id, { mode: "run", headers: p.who.headers })),
      "parent of helpers",
    );
    expect(parent.some((f) => f.event.activityType === "vymalo.session")).toBe(true);
    for (const e of await p.of(id, "child_session_opened")) {
      const child = validFrames(
        await frames(
          await connect(mock.base, String(e.data.thread), { mode: "run", headers: p.who.headers }),
        ),
        `child ${e.data.session}`,
      );
      expect(
        child.some(
          (f) =>
            f.event.activityType === "vymalo.job" && (f.event.content as { round?: unknown }).round,
        ),
      ).toBe(true);
      expect(child.some((f) => f.event.messageId === "session-self")).toBe(true);
    }
  });

  it("a failed helper, and a start that is refused", async () => {
    const p = await person();
    const id = newId();
    await (await run(mock, p.who, "adam", id, "helpers-fail go")).text();
    await p.reaches(id, ["done"]);
    const failed = validFrames(
      await frames(await connect(mock.base, id, { mode: "run", headers: p.who.headers })),
      "parent of helpers-fail",
    );
    expect(
      failed.some((f) => f.event.type === "SUBAGENT_ERROR" && f.event.code === "session_failed"),
    ).toBe(true);

    const refusing = await person("sessionStarts=refuse");
    const parent = await refusing.answered();
    await refusing.start(parent);
    await refusing.until(
      async () => (await refusing.of(parent, "child_session_start_ended")).length === 1,
      "the outcome",
    );
    const read = validFrames(
      await frames(
        await connect(mock.base, parent, { mode: "run", headers: refusing.who.headers }),
      ),
      "parent of a refused start",
    );
    const cards = read.filter((f) => f.event.activityType === "vymalo.sessionStart");
    expect(cards.map((f) => (f.event.content as { state: string }).state)).toEqual([
      "starting",
      "failed",
    ]);
    expect((cards[1]?.event.content as { reason: string } | undefined)?.reason).toContain(
      "max_working",
    );
  });

  it("a started subagent: the parent's stream, and the child's from the prompt to the close", async () => {
    const p = await person();
    const parent = await p.answered();
    const { child } = await p.opened(parent, { keepOpen: true });
    await p.until(async () => (await p.thread(child)).childOf?.state === "idle", "idle");
    await documented(
      "/api/threads/{threadId}",
      "get",
      await p.json("GET", `/api/threads/${child}`),
    );
    await p.close(child);
    validFrames(
      await frames(await connect(mock.base, parent, { mode: "run", headers: p.who.headers })),
      "parent of a start",
    );
    const read = validFrames(
      await frames(await connect(mock.base, child, { mode: "run", headers: p.who.headers })),
      "child of a start",
    );
    expect(read.filter((f) => f.event.messageId === "session-self").length).toBeGreaterThanOrEqual(
      2,
    );
  });
});
