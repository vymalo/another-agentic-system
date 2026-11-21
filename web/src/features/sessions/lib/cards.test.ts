import { describe, expect, it } from "vitest";
import { cardNewer, childOf, parseSession, parseSessionStart } from "./cards";

/*
 * The cards a child session is told by (docs/api/agui.md "Child sessions as subagents" and "A person
 * starts, sends and closes a session"): the parent's `vymalo.session`, the child's own `session-self`
 * (no `threadId`), `vymalo.sessionStart`, and `childOf` of a thread. The enums are closed, unknown
 * members are ignored, a bad shape is `null`, and a `threadId` that is not a UUID gives no link.
 */

const CHILD = "00000000-0000-7000-8000-000000000003";
const REQUEST = "00000000-0000-7000-8000-0000000000a1";

const parent = {
  session: "s1",
  threadId: CHILD,
  agent: "researcher",
  description: "Find the writers",
  keepOpen: true,
  state: "working",
  round: 2,
  report: "blocked",
  at: "2027-01-15T08:00:05Z",
};

describe("parseSession: the parent's card", () => {
  it("reads every member", () => {
    expect(parseSession(parent)).toEqual({
      session: "s1",
      threadId: CHILD,
      agent: "researcher",
      description: "Find the writers",
      keepOpen: true,
      state: "working",
      round: 2,
      report: "blocked",
      at: "2027-01-15T08:00:05Z",
      closing: false,
      closeAsked: false,
    });
  });

  it("says who started the session only for a person's start", () => {
    expect(parseSession({ ...parent, by: "person" })?.by).toBe("person");
    expect(parseSession(parent)).not.toHaveProperty("by");
  });

  it("leaves out the round and the report a card does not have yet", () => {
    const { round: _r, report: _p, ...fresh } = parent;
    const card = parseSession({ ...fresh, state: "idle" });
    expect(card).not.toBeNull();
    expect(card).not.toHaveProperty("round");
    expect(card).not.toHaveProperty("report");
  });

  it.each(["working", "idle", "closed"])("takes the state %s", (state) => {
    expect(parseSession({ ...parent, state })?.state).toBe(state);
  });

  it.each(["done", "failed", "canceled", "blocked"])("takes the report %s", (report) => {
    expect(parseSession({ ...parent, report })?.report).toBe(report);
  });

  it("ignores members it does not know", () => {
    expect(parseSession({ ...parent, later: { x: 1 } })).not.toBeNull();
    expect(parseSession({ ...parent, later: 1 })).not.toHaveProperty("later");
  });

  it.each([
    ["a state that is not one", { state: "paused" }],
    ["no state", { state: undefined }],
    ["a report that is not one", { report: "cancelled" }],
    ["a report that is not a string", { report: 3 }],
    ["a by that is not one", { by: "robot" }],
    ["a round of zero", { round: 0 }],
    ["a round that is fractional", { round: 1.5 }],
    ["a round that is a string", { round: "2" }],
    ["no session", { session: undefined }],
    ["an empty session", { session: "" }],
    ["a session that is a number", { session: 4 }],
    ["no agent", { agent: undefined }],
    ["a keepOpen that is not a boolean", { keepOpen: "yes" }],
    ["a description that is not a string", { description: 7 }],
    ["a closing that is not a boolean", { closing: "no" }],
  ])("is null for %s", (_label, change) => {
    expect(parseSession({ ...parent, ...change })).toBeNull();
  });

  it.each([null, undefined, "card", 3, [parent]])("is null for %j", (value) => {
    expect(parseSession(value)).toBeNull();
  });

  it("gives no link for a threadId that is not a UUID, and still reads the card", () => {
    for (const threadId of ["not-a-uuid", "../../x", 5, "", `${CHILD}/x`]) {
      const card = parseSession({ ...parent, threadId });
      expect(card).not.toBeNull();
      expect(card).not.toHaveProperty("threadId");
    }
  });

  it("takes a UUID of either case", () => {
    expect(parseSession({ ...parent, threadId: CHILD.toUpperCase() })?.threadId).toBe(
      CHILD.toUpperCase(),
    );
  });

  it("keeps the agent's words as they are: they are text, never markup", () => {
    const card = parseSession({ ...parent, agent: "<b>x</b>", description: "[a](javascript:1)" });
    expect(card?.agent).toBe("<b>x</b>");
    expect(card?.description).toBe("[a](javascript:1)");
  });
});

describe("parseSession: the child's own card (session-self)", () => {
  const own = {
    session: "s1",
    agent: "helper",
    keepOpen: true,
    state: "idle",
    closing: false,
    closeAsked: true,
    at: "2027-01-15T08:00:13Z",
  };

  it("has no threadId and no description, and says whether a close is on its way", () => {
    const card = parseSession(own);
    expect(card).toEqual({
      session: "s1",
      agent: "helper",
      description: "",
      keepOpen: true,
      state: "idle",
      closing: false,
      closeAsked: true,
      at: "2027-01-15T08:00:13Z",
    });
    expect(card).not.toHaveProperty("threadId");
  });

  it("reads closing and closeAsked, which are false when the card leaves them out", () => {
    expect(parseSession({ ...own, closing: true, closeAsked: undefined })).toMatchObject({
      closing: true,
      closeAsked: false,
    });
  });
});

describe("parseSessionStart", () => {
  const start = {
    request: REQUEST,
    agent: "planner",
    description: "Plan the work",
    keepOpen: false,
    state: "starting",
    at: "2027-01-15T08:00:06Z",
  };

  it("reads a start that waits", () => {
    expect(parseSessionStart(start)).toEqual({
      request: REQUEST,
      agent: "planner",
      description: "Plan the work",
      keepOpen: false,
      state: "starting",
      at: "2027-01-15T08:00:06Z",
    });
  });

  it("reads an opened start with the thread and the session", () => {
    expect(
      parseSessionStart({ ...start, state: "opened", threadId: CHILD, session: "s1" }),
    ).toMatchObject({ state: "opened", threadId: CHILD, session: "s1" });
  });

  it("reads a failed start with the agent's words", () => {
    expect(
      parseSessionStart({
        ...start,
        state: "failed",
        reason: "max_working: 2 helpers are working",
      }),
    ).toMatchObject({ state: "failed", reason: "max_working: 2 helpers are working" });
  });

  it("gives no link for a threadId that is not a UUID", () => {
    const card = parseSessionStart({ ...start, state: "opened", threadId: "nope", session: "s1" });
    expect(card).not.toBeNull();
    expect(card).not.toHaveProperty("threadId");
  });

  it.each([
    ["a state that is not one", { state: "running" }],
    ["no state", { state: undefined }],
    ["no request", { request: undefined }],
    ["an empty request", { request: "" }],
    ["no agent", { agent: undefined }],
    ["no description", { description: undefined }],
    ["a keepOpen that is not a boolean", { keepOpen: 0 }],
    ["a reason that is not a string", { reason: {} }],
    ["a session that is not a string", { session: 1 }],
  ])("is null for %s", (_label, change) => {
    expect(parseSessionStart({ ...start, ...change })).toBeNull();
  });

  it.each([null, undefined, "x", [start]])("is null for %j", (value) => {
    expect(parseSessionStart(value)).toBeNull();
  });
});

describe("childOf", () => {
  const row = {
    threadId: "00000000-0000-7000-8000-000000000001",
    session: "s1",
    agent: "researcher",
    state: "idle",
    keepOpen: true,
    closing: true,
    closeAsked: true,
  };

  it("reads the helper a child thread is a session of", () => {
    expect(childOf({ id: CHILD, childOf: row })).toEqual({
      parent: "00000000-0000-7000-8000-000000000001",
      session: "s1",
      agent: "researcher",
      state: "idle",
      keepOpen: true,
      closing: true,
      closeAsked: true,
    });
  });

  it("is null for a thread that is not a helper's", () => {
    expect(childOf({ id: CHILD })).toBeNull();
    expect(childOf({ id: CHILD, childOf: undefined })).toBeNull();
    expect(childOf(null)).toBeNull();
  });

  it("treats what the thread leaves out as the defaults: not kept open, not closing", () => {
    const card = childOf({ id: CHILD, childOf: { threadId: row.threadId, session: "s1" } });
    expect(card).toMatchObject({ keepOpen: false, closing: false, closeAsked: false });
    expect(card).not.toHaveProperty("agent");
    expect(card).not.toHaveProperty("state");
  });

  it("gives no link to a parent that is not a UUID, and is still a child", () => {
    const card = childOf({ id: CHILD, childOf: { ...row, threadId: "x/y" } });
    expect(card).not.toBeNull();
    expect(card).not.toHaveProperty("parent");
  });

  it.each([
    ["a state that is not one", { state: "gone" }],
    ["no session", { session: undefined }],
    ["an empty session", { session: "" }],
    ["a keepOpen that is not a boolean", { keepOpen: "true" }],
    ["a closing that is not a boolean", { closing: 1 }],
    ["an agent that is not a string", { agent: 9 }],
  ])("is null for %s", (_label, change) => {
    expect(childOf({ id: CHILD, childOf: { ...row, ...change } })).toBeNull();
  });

  it("is null when childOf is not an object", () => {
    expect(childOf({ id: CHILD, childOf: "s1" })).toBeNull();
    expect(childOf({ id: CHILD, childOf: [row] })).toBeNull();
  });
});

describe("cardNewer", () => {
  it("counts a card stamped the same instant as the resource as newer", () => {
    expect(cardNewer({ at: "2026-11-20T10:00:00Z" }, "2026-11-20T10:00:00Z")).toBe(true);
    expect(cardNewer({ at: "2026-11-20T10:00:00Z" }, "2026-11-20T10:00:01Z")).toBe(false);
  });
});
