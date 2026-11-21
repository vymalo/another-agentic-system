import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { connect, frames } from "./agui-client";
import {
  CHILD_SESSIONS_URI,
  checkDescription,
  DEFAULT_OFFER,
  helpersScript,
  readOffer,
} from "./sessions";
import { calls, type Mock, newId, newRequest, run, startMock } from "./sessions-client";

/*
 * The mock's child sessions (ADR 0072, #332): the offer on the capabilities document, the child threads,
 * the scripted agent's helpers, what a person starts, says to and closes, and every refusal. The
 * contract's side (every answer and frame against docs/api/chat-api.yaml) is sessions.contract.test.ts.
 */

let mock: Mock;
beforeAll(async () => {
  mock = await startMock();
});
afterAll(async () => {
  await mock.close();
});

/** A person of its own with the calls they make. */
async function person(config?: string) {
  const who = await mock.session(config);
  return { who, ...calls(mock, who) };
}

describe("readOffer: the card's entry, as the contract reads it", () => {
  const agents = [
    { name: "planner", description: "Breaks a request into steps." },
    { name: "researcher", description: "Looks things up." },
  ];

  it("takes agents in the card's order and a limit, and ignores what it does not know", () => {
    expect(readOffer({ agents, maxOpen: 4, later: 1 })).toEqual({ agents, maxOpen: 4 });
    expect(readOffer(DEFAULT_OFFER)).toEqual(DEFAULT_OFFER);
  });

  it("counts a whole double, and 1 and 256 at the edges", () => {
    expect(readOffer({ agents, maxOpen: 16.0 })?.maxOpen).toBe(16);
    expect(readOffer({ agents, maxOpen: 1 })?.maxOpen).toBe(1);
    expect(readOffer({ agents, maxOpen: 256 })?.maxOpen).toBe(256);
  });

  it("is no offer for anything malformed: a bad card is a card that does not list it", () => {
    for (const bad of [
      null,
      [],
      {},
      { agents: [], maxOpen: 4 },
      { agents, maxOpen: 0 },
      { agents, maxOpen: 257 },
      { agents, maxOpen: 1.5 },
      { agents, maxOpen: "4" },
      { agents },
      { agents: [{ name: "a" }], maxOpen: 4 },
      { agents: [{ name: " ", description: "d" }], maxOpen: 4 },
      {
        agents: [
          { name: "a", description: "x" },
          { name: "a", description: "y" },
        ],
        maxOpen: 4,
      },
    ]) {
      expect(readOffer(bad), JSON.stringify(bad)).toBeNull();
    }
  });
});

describe("checkDescription: the server's rule", () => {
  it("trims, and counts code points", () => {
    expect(checkDescription("  Plan it ")).toEqual({ ok: true, value: "Plan it" });
    expect(checkDescription("😀".repeat(80))).toMatchObject({ ok: true });
    expect(checkDescription("😀".repeat(81))).toMatchObject({ ok: false });
    expect(checkDescription("a".repeat(80))).toMatchObject({ ok: true });
    expect(checkDescription("a".repeat(81))).toMatchObject({ ok: false });
  });

  it("refuses a missing, blank, multi-line, control or format description", () => {
    for (const bad of [undefined, 3, "", "  ", "a\nb", "a\u0000b", "a‮b", "a​b", "﻿x"]) {
      expect(checkDescription(bad), JSON.stringify(bad)).toMatchObject({ ok: false });
    }
  });
});

describe("the helpers scripts are chosen by the first word", () => {
  it("has `helpers` and `helpers-fail`, and nothing for another word", () => {
    expect(helpersScript("helpers")?.start.length).toBeGreaterThan(5);
    expect(helpersScript("helpers-fail")?.start.length).toBeGreaterThan(2);
    expect(helpersScript("helper")).toBeUndefined();
    expect(helpersScript(undefined)).toBeUndefined();
  });
});

describe("the offer on the capabilities document", () => {
  const caps = async (p: Awaited<ReturnType<typeof person>>, agent: string) => {
    const res = await p.json("GET", `/agui/agents/${agent}/capabilities`);
    return ((await res.json()) as { custom?: Record<string, unknown> }).custom?.[
      CHILD_SESSIONS_URI
    ];
  };

  it("lists Adam's helpers and the limit, and no other agent's", async () => {
    const p = await person();
    expect(await caps(p, "adam")).toEqual({
      agents: [
        { name: "planner", description: "Breaks a request into steps." },
        { name: "researcher", description: "Looks things up on the web." },
      ],
      maxOpen: 4,
    });
    expect(await caps(p, "reviewer")).toBeUndefined();
  });

  it("is off for the session that turns it off, and for no other", async () => {
    const off = await person("sessions=off");
    const on = await person();
    expect(await caps(off, "adam")).toBeUndefined();
    expect(await caps(on, "adam")).toBeDefined();
    await off.config("sessions=on");
    expect(await caps(off, "adam")).toBeDefined();
  });

  it("refuses a switch that is not one", async () => {
    const p = await person();
    const res = await fetch(`${mock.base}/__mock/config?sessions=maybe&session=${p.who.name}`, {
      method: "POST",
    });
    expect(res.status).toBe(400);
    for (const q of ["sessionStarts=never", "sessionsNoted=0", "sessionsNoted=x"]) {
      const bad = await fetch(`${mock.base}/__mock/config?${q}&session=${p.who.name}`, {
        method: "POST",
      });
      expect(bad.status, q).toBe(400);
    }
  });
});

describe("the scripted agent: `helpers`", () => {
  it("opens a kept-open helper and one that closes when done, and reports them, as the parent's events", async () => {
    const p = await person();
    const id = newId();
    const res = await run(mock, p.who, "adam", id, "helpers go");
    expect(res.status).toBe(200);
    await res.text();
    // the late report and the close come after the agent is done
    await p.until(
      async () => (await p.of(id, "child_session_closed")).length === 2,
      "the late close of the helper that was kept open",
    );
    const log = await p.log(id);
    const sessions = log.filter((e) => e.kind.startsWith("child_session_"));
    expect(
      sessions.map((e) => [e.kind, e.data.agent ?? e.data.report ?? "", e.data.round ?? ""]),
    ).toEqual([
      ["child_session_opened", "planner", 1],
      ["child_session_opened", "researcher", 1],
      ["child_session_reported", "blocked", 1],
      ["child_session_reported", "done", 1],
      ["child_session_closed", "", ""],
      ["child_session_reported", "done", 1],
      ["child_session_reported", "canceled", 2],
      ["child_session_reported", "blocked", 3],
      ["child_session_reported", "done", 3],
      ["child_session_closed", "", ""],
    ]);
    // attributed to the parent's agent, once each, with who started them
    expect(sessions.every((e) => e.actor.type === "agent" && e.actor.name === "adam")).toBe(true);
    expect(sessions.filter((e) => e.kind === "child_session_opened").map((e) => e.data.by)).toEqual(
      ["agent", "agent"],
    );
    // the agent finished before the late events: the thread is done and they came after its end
    const done = log.findIndex((e) => e.kind === "thread_state" && e.data.state === "done");
    const late = log.findIndex((e, i) => i > done && e.kind === "child_session_reported");
    expect(done).toBeGreaterThan(0);
    expect(late).toBeGreaterThan(done);
  });

  it("makes a child thread for each helper, nested under the parent and titled by what it is for", async () => {
    const p = await person();
    const id = newId();
    await (await run(mock, p.who, "adam", id, "helpers go")).text();
    await p.until(async () => (await p.of(id, "child_session_closed")).length === 2, "the closes");
    const opened = await p.of(id, "child_session_opened");
    const children = await Promise.all(opened.map((e) => p.thread(String(e.data.thread))));
    expect(children.map((c) => c.title)).toEqual(["Plan the migration", "Check the sources"]);
    for (const c of children) {
      expect(c.nestedUnder).toBe(id);
      expect(c.target.agentId).toBe("adam");
      expect(c.childOf?.threadId).toBe(id);
    }
    expect(children.map((c) => [c.childOf?.agent, c.childOf?.state, c.childOf?.keepOpen])).toEqual([
      ["planner", "closed", true],
      ["researcher", "closed", false],
    ]);
    // the list nests each under its parent, and the parent is a row of its own
    const list = (await (await p.json("GET", "/api/threads?order=rail")).json()) as {
      id: string;
    }[];
    expect(list.map((t) => t.id)).toEqual([id, ...children.map((c) => c.id).reverse()]);
    // and the parent is no helper's
    expect((await p.thread(id)).childOf).toBeUndefined();
  });

  it("writes each round of a helper into its child thread's log, a round a job", async () => {
    const p = await person();
    const id = newId();
    await (await run(mock, p.who, "adam", id, "helpers go")).text();
    await p.until(async () => (await p.of(id, "child_session_closed")).length === 2, "the closes");
    const [plan, sources] = (await p.of(id, "child_session_opened")).map((e) =>
      String(e.data.thread),
    );
    const rounds = (await p.of(plan as string, "job_started")).map((e) => [
      e.data.job,
      e.data.round,
    ]);
    expect(rounds).toEqual([
      [1, { number: 1, by: "agent" }],
      [2, { number: 2, by: "agent" }],
      [3, { number: 3, by: "agent" }],
    ]);
    // the round that was blocked said so, and went on
    const sourcesLog = await p.kinds(sources as string);
    expect(sourcesLog).toContain("thread_state");
    const states = (await p.of(sources as string, "thread_state")).map((e) => e.data.state);
    expect(states).toEqual(["blocked", "done"]);
    // the child says its own session's states, once each as they change: a helper that closes when done closes
    const said = (await p.of(sources as string, "child_session_state")).map((e) => [
      e.data.state,
      e.data.closing,
    ]);
    expect(said).toEqual([["closed", false]]);
    const planSaid = (await p.of(plan as string, "child_session_state")).map((e) => e.data.state);
    expect(planSaid).toEqual(["idle", "working", "idle", "working", "idle", "closed"]);
  });

  it("`helpers-fail`: a helper whose round fails is reported failed and closes", async () => {
    const p = await person();
    const id = newId();
    await (await run(mock, p.who, "adam", id, "helpers-fail go")).text();
    await p.reaches(id, ["done"]);
    const events = (await p.log(id)).filter((e) => e.kind.startsWith("child_session_"));
    expect(events.map((e) => [e.kind, e.data.report ?? ""])).toEqual([
      ["child_session_opened", ""],
      ["child_session_reported", "failed"],
      ["child_session_closed", ""],
    ]);
    const child = await p.thread(String(events[0]?.data.thread));
    expect(child.state).toBe("failed");
    expect(child.childOf?.state).toBe("closed");
  });

  it("tells the helper in the parent's stream: a subagent named after it, a card, and the child thread in its metadata", async () => {
    const p = await person();
    const id = newId();
    await (await run(mock, p.who, "adam", id, "helpers go")).text();
    await p.until(async () => (await p.of(id, "child_session_closed")).length === 2, "the closes");
    const read = await frames(
      await connect(mock.base, id, { mode: "run", headers: p.who.headers }),
    );
    const started = read.filter(
      (f) =>
        f.event.type === "SUBAGENT_STARTED" &&
        String(f.event.subagentRunId).startsWith("sub-session-"),
    );
    expect(started.length).toBeGreaterThanOrEqual(2);
    const planStarted = started[0]?.event;
    expect(planStarted?.name).toBe("planner");
    expect(
      (planStarted?.metadata as Record<string, unknown> | undefined)?.["vymalo.childThread"],
    ).toMatch(/^[0-9a-f-]{36}$/);
    const cards = read.filter(
      (f) => f.event.type === "ACTIVITY_SNAPSHOT" && f.event.activityType === "vymalo.session",
    );
    expect(cards.length).toBeGreaterThanOrEqual(6);
    expect(cards.every((f) => f.event.replace === true)).toBe(true);
    const last = cards.filter((f) => f.event.messageId === "session-4").at(-1)?.event;
    expect((last?.content as Record<string, unknown> | undefined)?.state).toBe("closed");
  });
});

describe("a person starts a subagent (`startChildSession`)", () => {
  it("answers 202 with the request and Location, writes the request at once, and opens a child thread with the prompt", async () => {
    const p = await person();
    const parent = await p.answered();
    const request = newRequest();
    const res = await p.start(parent, {
      id: request,
      agent: "researcher",
      description: "  Find the writers ",
      text: "List the writers of the session header",
      keepOpen: true,
    });
    expect(res.status).toBe(202);
    expect(res.headers.get("Location")).toBe(`/api/threads/${parent}`);
    expect(await res.json()).toEqual({ request });
    // written before the answer: the request, with no text
    const asked = (await p.of(parent, "child_session_start_asked")).at(-1);
    expect(asked?.actor).toEqual({ type: "user", name: p.who.user });
    expect(asked?.data).toEqual({
      request,
      agent: "researcher",
      description: "Find the writers",
      keepOpen: true,
    });
    await p.until(
      async () => (await p.of(parent, "child_session_start_ended")).length === 1,
      "the outcome",
    );
    // the parent's events, in the order the orchestrator writes them
    const kinds = (await p.log(parent))
      .map((e) => e.kind)
      .filter((k) => k.startsWith("child_session_"))
      .slice(0, 3);
    expect(kinds).toEqual([
      "child_session_start_asked",
      "child_session_opened",
      "child_session_start_ended",
    ]);
    const opened = (await p.of(parent, "child_session_opened"))[0];
    expect(opened?.data).toMatchObject({
      agent: "researcher",
      description: "Find the writers",
      keepOpen: true,
      by: "person",
      round: 1,
    });
    const ended = (await p.of(parent, "child_session_start_ended"))[0];
    expect(ended?.actor.type).toBe("system");
    expect(ended?.data).toMatchObject({
      request,
      outcome: "opened",
      session: opened?.data.session,
      thread: opened?.data.thread,
    });
    // the child: its first message is the prompt, by the person
    const child = String(opened?.data.thread);
    const first = (await p.log(child))[0];
    expect(first?.kind).toBe("user_message");
    expect(first?.data.text).toBe("List the writers of the session header");
    expect(first?.actor.type).toBe("user");
    const thread = await p.thread(child);
    expect(thread).toMatchObject({
      title: "Find the writers",
      nestedUnder: parent,
      childOf: {
        threadId: parent,
        session: opened?.data.session,
        agent: "researcher",
        keepOpen: true,
      },
    });
    // the round runs and ends: kept open, so the session is idle, and the parent is told
    await p.reaches(child, ["done"]);
    await p.until(
      async () => (await p.thread(child)).childOf?.state === "idle",
      "the session to be idle",
    );
    const report = (await p.of(parent, "child_session_reported")).at(-1);
    expect(report?.data).toMatchObject({ session: opened?.data.session, round: 1, report: "done" });
  });

  it("a session that closes when done is closed when its round ends, and reported then closed", async () => {
    const p = await person();
    const parent = await p.answered();
    const { child } = await p.opened(parent, { keepOpen: false });
    await p.until(async () => (await p.thread(child)).childOf?.state === "closed", "the close");
    expect(
      (await p.log(parent))
        .filter((e) => e.kind.startsWith("child_session_"))
        .map((e) => e.kind)
        .slice(-2),
    ).toEqual(["child_session_reported", "child_session_closed"]);
  });

  it("the same request again is 202 and writes nothing", async () => {
    const p = await person();
    const parent = await p.answered();
    const request = newRequest();
    expect((await p.start(parent, { id: request })).status).toBe(202);
    await p.until(
      async () => (await p.of(parent, "child_session_start_ended")).length === 1,
      "the outcome",
    );
    // the round runs to its end, and the parent is told, before the count
    await p.until(
      async () => (await p.of(parent, "child_session_reported")).length === 1,
      "the round to be reported",
    );
    const before = (await p.log(parent)).length;
    const again = await p.start(parent, { id: request });
    expect(again.status).toBe(202);
    expect(await again.json()).toEqual({ request });
    await new Promise((r) => setTimeout(r, 60));
    expect((await p.log(parent)).length).toBe(before);
    expect(await p.of(parent, "child_session_opened")).toHaveLength(1);
  });

  it("a start the agent refuses ends `failed` in the agent's own words, and makes no thread", async () => {
    const p = await person("sessionStarts=refuse");
    const parent = await p.answered();
    expect((await p.start(parent)).status).toBe(202);
    await p.until(
      async () => (await p.of(parent, "child_session_start_ended")).length === 1,
      "the outcome",
    );
    const ended = (await p.of(parent, "child_session_start_ended"))[0];
    expect(ended?.data).toEqual({
      request: expect.any(String),
      outcome: "failed",
      reason: "max_working: 4 rounds are already working in this context; wait for one to finish",
    });
    expect(await p.of(parent, "child_session_opened")).toHaveLength(0);
    const list = (await (await p.json("GET", "/api/threads")).json()) as { id: string }[];
    expect(list.map((t) => t.id)).toEqual([parent]);
  });

  it("the agent's limit of open sessions is its own refusal, after the offer's `maxOpen` sessions are open", async () => {
    const p = await person();
    const parent = await p.answered();
    for (let i = 0; i < 4; i++) await p.opened(parent, { keepOpen: true, text: `hold ${i}` });
    const request = newRequest();
    expect((await p.start(parent, { id: request })).status).toBe(202);
    await p.until(
      async () => (await p.of(parent, "child_session_start_ended")).length === 5,
      "the fifth outcome",
    );
    const fifth = (await p.of(parent, "child_session_start_ended")).at(-1);
    expect(fifth?.data).toMatchObject({ request, outcome: "failed" });
    expect(String(fifth?.data.reason)).toContain("4 sessions are open");
  });

  it("`sessionStarts=hold` keeps starts waiting until a test releases them, four at most", async () => {
    const p = await person("sessionStarts=hold");
    const parent = await p.answered();
    for (let i = 0; i < 4; i++) expect((await p.start(parent)).status).toBe(202);
    await new Promise((r) => setTimeout(r, 40));
    expect(await p.of(parent, "child_session_start_ended")).toHaveLength(0);
    const fifth = await p.start(parent);
    expect(fifth.status).toBe(409);
    expect(((await fifth.json()) as { code: string }).code).toBe("too_many_starts");
    await p.release(parent);
    await p.until(
      async () => (await p.of(parent, "child_session_start_ended")).length === 4,
      "the four outcomes",
    );
    // and once they have ended, a start is taken again
    await p.config("sessionStarts=open");
    expect((await p.start(parent)).status).toBe(202);
  });
});

describe("what a start is refused for", () => {
  const code = async (res: Response) => ((await res.json()) as { code?: string }).code;

  it("is refused in the server's order, before anything is written", async () => {
    const p = await person();
    const parent = await p.answered();
    const before = (await p.log(parent)).length;
    const base = { id: newRequest(), agent: "planner", description: "Plan", text: "Plan it" };
    const cases: [string, Record<string, unknown> | string, number, string | undefined][] = [
      ["not JSON", "{nope", 400, undefined],
      ["an unknown member", { ...base, extra: 1 }, 400, undefined],
      ["no id", { agent: "planner", description: "x", text: "y" }, 400, undefined],
      ["an id that is not a UUID", { ...base, id: "nope" }, 400, undefined],
      ["keepOpen that is not a boolean", { ...base, keepOpen: "yes" }, 400, undefined],
      ["a tier that is not one of the four", { ...base, tier: "root" }, 400, undefined],
      ["no text", { id: base.id, agent: "planner", description: "x" }, 400, undefined],
      ["an unknown helper", { ...base, agent: "nobody" }, 422, "unknown_helper"],
      ["no description", { id: base.id, agent: "planner", text: "x" }, 422, "bad_description"],
      ["a blank description", { ...base, description: "   " }, 422, "bad_description"],
      [
        "a description of 81 characters",
        { ...base, description: "a".repeat(81) },
        422,
        "bad_description",
      ],
      ["a description on two lines", { ...base, description: "a\nb" }, 422, "bad_description"],
      [
        "a description with a control character",
        { ...base, description: "a\u0000b" },
        422,
        "bad_description",
      ],
      ["empty text", { ...base, text: "" }, 400, undefined],
      ["blank text", { ...base, text: "  \n" }, 400, undefined],
      ["text of 100001 characters", { ...base, text: "x".repeat(100_001) }, 400, undefined],
    ];
    for (const [label, body, status, expected] of cases) {
      const res = await p.json("POST", `/api/threads/${parent}/sessions`, body);
      expect(res.status, label).toBe(status);
      expect(await code(res), label).toBe(expected);
    }
    expect((await p.log(parent)).length).toBe(before);
  });

  it("names the server's first refusal when a body has two faults", async () => {
    const p = await person();
    const parent = await p.answered();
    const base = { id: newRequest(), agent: "planner", description: "Plan", text: "Plan it" };
    const start = (thread: string, body: Record<string, unknown>) =>
      p.json("POST", `/api/threads/${thread}/sessions`, body);
    // the text (400) is read before the description (422), which is read before the card's helper
    let res = await start(parent, { ...base, text: "", description: "", agent: "nobody" });
    expect([res.status, await code(res)]).toEqual([400, undefined]);
    res = await start(parent, { ...base, description: "", agent: "nobody" });
    expect([res.status, await code(res)]).toEqual([422, "bad_description"]);
    res = await start(parent, { ...base, agent: "nobody", tier: "plan" });
    expect([res.status, await code(res)]).toEqual([422, "unknown_helper"]);
    // a helper's own thread is refused before the text is read
    const { child } = await p.opened(parent);
    res = await start(child, { ...base, text: "" });
    expect([res.status, await code(res)]).toEqual([422, "child_thread"]);
    // the description is read before the card says it offers nothing
    const off = await person("sessions=off");
    const offParent = await off.answered();
    res = await off.json("POST", `/api/threads/${offParent}/sessions`, {
      ...base,
      description: "",
    });
    expect([res.status, await code(res)]).toEqual([422, "bad_description"]);
  });

  it("is 404 for a thread that is not there or not the person's, and 403 without the permission", async () => {
    const p = await person();
    const other = await person();
    const parent = await p.answered();
    expect((await p.start(newId())).status).toBe(404);
    expect((await other.start(parent)).status).toBe(404);
    const reader = await person("me=read-only");
    expect((await reader.start(parent)).status).toBe(403);
  });

  it("is 422 `no_child_sessions` for an agent whose card does not list the extension", async () => {
    const p = await person("sessions=off");
    const parent = await p.answered();
    const res = await p.start(parent);
    expect(res.status).toBe(422);
    expect(await code(res)).toBe("no_child_sessions");
  });

  it("is 409 `no_context` for a thread the agent has not answered", async () => {
    const slow = await startMock(60_000);
    try {
      const who = await slow.session();
      const q = calls(slow, who);
      const id = newId();
      const res = await run(slow, who, "adam", id, "echo hello");
      expect(res.status).toBe(200);
      const refused = await q.start(id);
      expect(refused.status).toBe(409);
      expect(await code(refused)).toBe("no_context");
    } finally {
      await slow.close();
    }
  });

  it("is 409 `too_many_sessions` past the most a thread notes", async () => {
    const p = await person("sessionsNoted=2");
    const parent = await p.answered();
    await p.opened(parent);
    await p.opened(parent);
    const third = await p.start(parent);
    expect(third.status).toBe(409);
    expect(await code(third)).toBe("too_many_sessions");
  });

  it("is 422 `child_thread` from a subagent's own thread", async () => {
    const p = await person();
    const parent = await p.answered();
    const { child } = await p.opened(parent);
    const res = await p.start(child);
    expect(res.status).toBe(422);
    expect(await code(res)).toBe("child_thread");
  });

  it("is 422 for a tier the card does not honour, and 400 for one that is not a tier", async () => {
    const p = await person();
    const parent = await p.answered();
    // the card of `adam` lists no tiers in this session
    const res = await p.start(parent, { tier: "plan" });
    expect(res.status).toBe(422);
  });
});

describe("closing a subagent (`closeChildSession`)", () => {
  it("closes an idle session at once: the child says it asked and closed, the parent says closed, once", async () => {
    const p = await person();
    const parent = await p.answered();
    const { child, session } = await p.opened(parent, { keepOpen: true });
    await p.until(async () => (await p.thread(child)).childOf?.state === "idle", "idle");
    const res = await p.close(child);
    expect(res.status).toBe(202);
    expect(await res.json()).toEqual({ state: "closed" });
    const asked = (await p.of(child, "child_session_close_asked")).at(-1);
    expect(asked?.actor.type).toBe("user");
    expect(asked?.data).toEqual({ session, cancel: false });
    const state = (await p.of(child, "child_session_state")).at(-1);
    expect(state?.data).toEqual({ session, state: "closed", closing: false, closeAsked: true });
    expect(await p.of(parent, "child_session_closed")).toHaveLength(1);
    expect((await p.thread(child)).childOf).toMatchObject({ state: "closed", closeAsked: true });
    // a second close is 202 and writes nothing
    const before = (await p.log(child)).length;
    const again = await p.close(child);
    expect(again.status).toBe(202);
    expect(await again.json()).toEqual({ state: "closed" });
    expect((await p.log(child)).length).toBe(before);
    expect(await p.of(parent, "child_session_closed")).toHaveLength(1);
  });

  it("closes a working session when its round ends, and with `cancel` stops the round first", async () => {
    const p = await person();
    const parent = await p.answered();
    const waiting = await p.opened(parent, { keepOpen: true, text: "hold on" });
    await p.reaches(waiting.child, ["working"]);
    const res = await p.close(waiting.child);
    expect(res.status).toBe(202);
    expect(await res.json()).toEqual({ state: "closing" });
    expect((await p.thread(waiting.child)).childOf).toMatchObject({
      state: "working",
      closing: true,
      closeAsked: true,
    });
    // the same close again, or a weaker one, is nothing
    const said = async () =>
      (await p.of(waiting.child, "child_session_close_asked", "child_session_state")).length;
    const before = await said();
    expect(await (await p.close(waiting.child)).json()).toEqual({ state: "closing" });
    expect(await said()).toBe(before);
    // the round ends: the session closes
    await p.release(waiting.child);
    await p.until(
      async () => (await p.thread(waiting.child)).childOf?.state === "closed",
      "closed",
    );
    expect((await p.of(parent, "child_session_reported")).at(-1)?.data.report).toBe("done");
    expect(await p.of(parent, "child_session_closed")).toHaveLength(1);

    // with `cancel`: the round is stopped, reported canceled, and the session closes
    const second = await p.opened(parent, { keepOpen: true, text: "hold on" });
    await p.reaches(second.child, ["working"]);
    const stopped = await p.close(second.child, { cancel: true });
    expect(await stopped.json()).toEqual({ state: "closing" });
    await p.until(async () => (await p.thread(second.child)).childOf?.state === "closed", "closed");
    expect((await p.thread(second.child)).state).toBe("cancelled");
    expect((await p.of(parent, "child_session_reported")).at(-1)?.data.report).toBe("canceled");
    expect(await p.of(parent, "child_session_closed")).toHaveLength(2);
  });

  it("a stronger close asks again: `cancel` after a close that waits stops the round", async () => {
    const p = await person();
    const parent = await p.answered();
    const { child } = await p.opened(parent, { keepOpen: true, text: "hold on" });
    await p.reaches(child, ["working"]);
    await p.close(child);
    const asked = (await p.of(child, "child_session_close_asked")).length;
    await p.close(child, { cancel: true });
    expect(await p.of(child, "child_session_close_asked")).toHaveLength(asked + 1);
    await p.until(async () => (await p.thread(child)).childOf?.state === "closed", "closed");
  });

  it("is refused: 400 for a body that is not `{cancel}`, 422 for a thread that is no subagent's or whose card is gone", async () => {
    const p = await person();
    const parent = await p.answered();
    const { child } = await p.opened(parent, { keepOpen: true });
    for (const body of ['{"cancel":"yes"}', '{"cancel":true,"x":1}', "[]", "{nope"]) {
      expect((await p.close(child, body)).status, body).toBe(400);
    }
    const notChild = await p.close(parent);
    expect(notChild.status).toBe(422);
    expect(((await notChild.json()) as { code: string }).code).toBe("not_child_thread");
    expect((await p.close(newId())).status).toBe(404);
    expect((await (await person()).close(child)).status).toBe(404);
    await p.until(async () => (await p.thread(child)).childOf?.state === "idle", "idle");
    await p.config("sessions=off");
    const gone = await p.close(child);
    expect(gone.status).toBe(422);
    expect(((await gone.json()) as { code: string }).code).toBe("no_child_sessions");
    // a session that is closed already is 202 whatever the card says
    await p.config("sessions=on");
    await p.close(child);
    await p.config("sessions=off");
    expect((await p.close(child)).status).toBe(202);
    expect((await (await person("me=read-only")).close(child)).status).toBe(403);
  });
});

describe("a person talks to a subagent (`runAgent` on a child thread)", () => {
  const send = (
    p: Awaited<ReturnType<typeof person>>,
    child: string,
    text: string,
    props: Record<string, unknown> = {},
  ) => run(mock, p.who, "adam", child, text, props);

  it("an idle session takes the message as its next round, and is reported to the parent", async () => {
    const p = await person();
    const parent = await p.answered();
    const { child } = await p.opened(parent, { keepOpen: true });
    await p.until(async () => (await p.thread(child)).childOf?.state === "idle", "idle");
    const res = await send(p, child, "Now the editors");
    expect(res.status).toBe(200);
    await res.text();
    await p.until(
      async () => (await p.of(parent, "child_session_reported")).length === 2,
      "round 2",
    );
    const log = await p.log(child);
    expect(log.find((e) => e.kind === "job_started" && e.data.job === 2)).toBeDefined();
    expect(log.filter((e) => e.kind === "agent_message").map((e) => e.data.text)).toEqual([
      "Round 1: Plan it",
      "Round 2: Now the editors",
    ]);
    expect((await p.of(parent, "child_session_reported")).at(-1)?.data).toMatchObject({
      round: 2,
      report: "done",
    });
    expect((await p.thread(child)).childOf?.state).toBe("idle");
  });

  it("a working session takes a steer into its round", async () => {
    const p = await person();
    const parent = await p.answered();
    const { child } = await p.opened(parent, { keepOpen: true, text: "hold on" });
    await p.reaches(child, ["working"]);
    const res = await send(p, child, "Include the translators", { "vymalo.send": "steer" });
    expect(res.status).toBe(200);
    await p.until(
      async () =>
        (await p.of(child, "agent_message")).some(
          (e) => e.data.text === "steered: Include the translators",
        ),
      "the steer to be read",
    );
    expect((await p.of(child, "user_message")).at(-1)?.data.delivery).toBe("steer");
    await p.release(child);
    await p.reaches(child, ["done"]);
  });

  it("without a way to deliver it, a message while the round works is refused", async () => {
    const p = await person();
    const parent = await p.answered();
    const { child } = await p.opened(parent, { keepOpen: true, text: "hold on" });
    await p.reaches(child, ["working"]);
    expect((await send(p, child, "more")).status).toBe(409);
  });

  it("Stop & send on a kept-open session cancels the round, then starts the next", async () => {
    const p = await person();
    const parent = await p.answered();
    const { child } = await p.opened(parent, { keepOpen: true, text: "hold on" });
    await p.reaches(child, ["working"]);
    const res = await send(p, child, "Start over", { "vymalo.send": "interrupt" });
    expect(res.status).toBe(200);
    await p.until(
      async () => (await p.of(parent, "child_session_reported")).length === 2,
      "the second round to be reported",
    );
    const reports = (await p.of(parent, "child_session_reported")).map((e) => [
      e.data.round,
      e.data.report,
    ]);
    expect(reports).toEqual([
      [1, "canceled"],
      [2, "done"],
    ]);
    const statuses = (await p.of(child, "agent_status")).map((e) => e.data.status);
    expect(statuses).toContain("canceled");
    expect((await p.of(child, "agent_message")).map((e) => e.data.text)).toEqual([
      "Round 2: Start over",
    ]);
  });

  it("is refused: a closed session (409), a message that mentions agents (422), and Stop & send on one that closes when done (422)", async () => {
    const p = await person();
    const parent = await p.answered();
    const kept = await p.opened(parent, { keepOpen: true });
    await p.until(async () => (await p.thread(kept.child)).childOf?.state === "idle", "idle");
    const mention = [{ agentId: "reviewer", label: "@reviewer", start: 0, end: 9 }];
    const mentioned = await send(p, kept.child, "@reviewer hi", { "vymalo.mentions": mention });
    expect(mentioned.status).toBe(422);
    expect(((await mentioned.json()) as { code: string }).code).toBe("child_mentions");

    const once = await p.opened(parent, { keepOpen: false, text: "hold on" });
    await p.reaches(once.child, ["working"]);
    const stop = await send(p, once.child, "again", { "vymalo.send": "interrupt" });
    expect(stop.status).toBe(422);
    expect(((await stop.json()) as { code: string }).code).toBe("session_closes_on_stop");
    await p.release(once.child);
    await p.until(async () => (await p.thread(once.child)).childOf?.state === "closed", "closed");
    const closed = await send(p, once.child, "hello again");
    expect(closed.status).toBe(409);
    expect(((await closed.json()) as { code: string }).code).toBe("session_closed");
    // nothing was written by any of them
    expect((await p.of(kept.child, "user_message")).map((e) => e.data.text)).toEqual(["Plan it"]);
  });

  it("another person's message to the thread is a 404", async () => {
    const p = await person();
    const other = await person();
    const parent = await p.answered();
    const { child } = await p.opened(parent, { keepOpen: true });
    expect((await send(other, child, "hello")).status).toBe(404);
  });

  it("Stop on a kept-open session leaves it idle, and on one that closes when done reports it canceled then closes it", async () => {
    const p = await person();
    const parent = await p.answered();
    const kept = await p.opened(parent, { keepOpen: true, text: "hold on" });
    await p.reaches(kept.child, ["working"]);
    expect((await p.json("POST", `/api/threads/${kept.child}/cancel`)).status).toBe(202);
    await p.reaches(kept.child, ["cancelled"]);
    await p.until(async () => (await p.thread(kept.child)).childOf?.state === "idle", "idle");
    // idle after a stop: it takes the next message
    expect((await send(p, kept.child, "again")).status).toBe(200);

    const once = await p.opened(parent, { keepOpen: false, text: "hold on" });
    await p.reaches(once.child, ["working"]);
    await p.json("POST", `/api/threads/${once.child}/cancel`);
    await p.until(async () => (await p.thread(once.child)).childOf?.state === "closed", "closed");
    const reports = await p.of(parent, "child_session_reported");
    expect(
      reports.some((e) => e.data.session === once.session && e.data.report === "canceled"),
    ).toBe(true);
    expect(await p.of(parent, "child_session_closed")).toHaveLength(1);
  });
});

describe("what is never done to a subagent's thread", () => {
  it("is not forked and not shared (422 `child_thread`), and it is deleted with its parent", async () => {
    const p = await person();
    const parent = await p.answered();
    const { child } = await p.opened(parent, { keepOpen: true });
    await p.reaches(child, ["done"]);
    const fork = await p.json("POST", `/api/threads/${child}/fork`, { after: 1 });
    expect(fork.status).toBe(422);
    expect(((await fork.json()) as { code: string }).code).toBe("child_thread");
    const share = await p.json("PUT", `/api/threads/${child}/share`, { visibility: "internal" });
    expect(share.status).toBe(422);
    expect(((await share.json()) as { code: string }).code).toBe("child_thread");
    // the parent can be shared as it always was
    expect(
      (await p.json("PUT", `/api/threads/${parent}/share`, { visibility: "internal" })).status,
    ).toBe(200);
    const gone = await p.json("DELETE", `/api/threads/${parent}`);
    expect(gone.status).toBe(204);
    expect((await p.json("GET", `/api/threads/${child}`)).status).toBe(404);
  });

  it("a parent whose subagent works is not deleted: it works, like any thread that does", async () => {
    const p = await person();
    const parent = await p.answered();
    const { child } = await p.opened(parent, { keepOpen: true, text: "hold on" });
    await p.reaches(child, ["working"]);
    const refused = await p.json("DELETE", `/api/threads/${parent}`);
    expect(refused.status).toBe(409);
    expect(((await refused.json()) as { code: string }).code).toBe("thread_active");
    expect((await p.json("GET", `/api/threads/${child}`)).status).toBe(200);
  });
});

describe("the streams of a started subagent", () => {
  it("the parent's stream says the start as a card, then the subagent; the child's stream says the round and its own state", async () => {
    const p = await person();
    const parent = await p.answered();
    const { child, request, session } = await p.opened(parent, { keepOpen: true });
    await p.reaches(child, ["done"]);
    await p.close(child);
    const parentFrames = await frames(
      await connect(mock.base, parent, { mode: "run", headers: p.who.headers }),
    );
    const starts = parentFrames.filter(
      (f) =>
        f.event.activityType === "vymalo.sessionStart" &&
        f.event.messageId === `session-start-${request}`,
    );
    expect(starts.map((f) => (f.event.content as Record<string, unknown>).state)).toEqual([
      "starting",
      "opened",
    ]);
    expect(starts[1]?.event.content).toMatchObject({ threadId: child, session, agent: "planner" });
    const subagent = parentFrames.find(
      (f) =>
        f.event.type === "SUBAGENT_STARTED" &&
        String(f.event.subagentRunId).startsWith("sub-session-"),
    );
    expect(subagent?.event.name).toBe("planner");
    const card = parentFrames.find(
      (f) =>
        f.event.activityType === "vymalo.session" &&
        (f.event.content as { by?: string }).by === "person",
    );
    expect(card).toBeDefined();

    const childFrames = await frames(
      await connect(mock.base, child, { mode: "run", headers: p.who.headers }),
    );
    const self = childFrames.filter((f) => f.event.messageId === "session-self");
    expect(self.length).toBeGreaterThanOrEqual(1);
    const last = self.at(-1)?.event.content as Record<string, unknown>;
    expect(last).toMatchObject({
      session,
      agent: "planner",
      keepOpen: true,
      state: "closed",
      closing: false,
      closeAsked: true,
    });
    expect(last).not.toHaveProperty("threadId");
  });

  it("a child thread's own card comes from its log: a viewer connected before the close reads it live", async () => {
    const p = await person();
    const parent = await p.answered();
    const { child } = await p.opened(parent, { keepOpen: true, text: "hold on" });
    await p.reaches(child, ["working"]);
    const live = connect(mock.base, child, { headers: p.who.headers });
    await p.close(child);
    await p.release(child);
    const read = await frames(
      await live,
      (f) =>
        f.event.messageId === "session-self" &&
        (f.event.content as { state?: string }).state === "closed",
    );
    const states = read
      .filter((f) => f.event.messageId === "session-self")
      .map((f) => (f.event.content as Record<string, unknown>).state);
    expect(states.at(-1)).toBe("closed");
  });
});

describe("a thread shared by a link", () => {
  it("never tells a reader of its helper sessions: their events are inert, and no card is drawn", async () => {
    const p = await person();
    const id = newId();
    await (await run(mock, p.who, "adam", id, "helpers go")).text();
    await p.until(async () => (await p.of(id, "child_session_closed")).length === 2, "the closes");
    const share = await p.json("PUT", `/api/threads/${id}/share`, { visibility: "internal" });
    expect(share.status).toBe(200);
    const url = ((await share.json()) as { url: string }).url;
    const token = url.replace("/s/", "");
    const exported = await (await p.json("GET", `/api/shared/${token}`)).json();
    expect(JSON.stringify(exported)).not.toContain("child_session");
    expect(JSON.stringify(exported)).not.toContain("childOf");
    const read = await frames(
      await fetch(`${mock.base}/agui/shared/${token}/connect?mode=run`, {
        headers: { ...p.who.headers, Accept: "text/event-stream" },
      }),
    );
    expect(read.length).toBeGreaterThan(3);
    expect(JSON.stringify(read)).not.toContain("vymalo.session");
    expect(JSON.stringify(read)).not.toContain("sub-session");
  });
});

describe("a started subagent is not the thread of a person who did not start it", () => {
  it("is in the owner's list and nobody else's", async () => {
    const p = await person();
    const other = await person();
    const parent = await p.answered();
    const { child } = await p.opened(parent);
    const list = (await (await other.json("GET", "/api/threads")).json()) as { id: string }[];
    expect(list.map((t) => t.id)).not.toContain(child);
    expect((await other.json("GET", `/api/threads/${child}`)).status).toBe(404);
  });
});
