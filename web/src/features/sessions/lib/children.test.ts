import { describe, expect, it } from "vitest";
import type { ApiThread } from "@/lib/api/types";
import type { SessionCard, SessionStartCard } from "./cards";
import { type AppliedEntry, liveCloses, mergeChildren, startsWaiting } from "./children";

const PARENT = "00000000-0000-4000-8000-000000000001";
const A = "00000000-0000-4000-8000-00000000000a";
const B = "00000000-0000-4000-8000-00000000000b";
const C = "00000000-0000-4000-8000-00000000000c";

const row = (id: string, session: string, over: Record<string, unknown> = {}): ApiThread =>
  ({
    id,
    title: `row ${session}`,
    state: "done",
    target: { agentId: "adam" },
    nestedUnder: PARENT,
    childOf: { threadId: PARENT, session, agent: "planner", state: "idle", keepOpen: true },
    ...over,
  }) as unknown as ApiThread;

const card = (session: string, over: Partial<SessionCard> = {}): SessionCard => ({
  session,
  threadId: A,
  agent: "researcher",
  description: `card ${session}`,
  keepOpen: false,
  state: "working",
  closing: false,
  closeAsked: false,
  ...over,
});

const start = (request: string, over: Partial<SessionStartCard> = {}): SessionStartCard => ({
  request,
  agent: "planner",
  description: `start ${request}`,
  keepOpen: false,
  state: "starting",
  ...over,
});

const merge = (input: Partial<Parameters<typeof mergeChildren>[0]> = {}) =>
  mergeChildren({
    parent: PARENT,
    rows: [],
    cards: new Map(),
    starts: new Map(),
    applied: new Map(),
    ...input,
  });

describe("the subagents of a thread, merged", () => {
  it("lists the rail rows that are this thread's children, and no other thread's", () => {
    const other = row(C, "s9", {
      childOf: { threadId: "00000000-0000-4000-8000-0000000000ff", session: "s9" },
    });
    const plain = { id: B, title: "plain", target: { agentId: "adam" } } as unknown as ApiThread;
    const m = merge({ rows: [row(A, "s1"), other, plain] });
    expect(m.open.map((i) => i.session)).toEqual(["s1"]);
    expect(m.open[0]?.threadId).toBe(A);
    expect(m.open[0]?.description).toBe("row s1");
  });

  it("adds the sessions only a card says, for a rail page that is not loaded", () => {
    const m = merge({
      rows: [row(A, "s1")],
      cards: new Map([["session-3", card("s2", { threadId: B })]]),
    });
    expect(m.open.map((i) => i.session)).toEqual(["s1", "s2"]);
    expect(m.open[1]?.threadId).toBe(B);
  });

  it("lets the row win on the state, over the card of the same session", () => {
    const m = merge({
      rows: [
        row(A, "s1", {
          childOf: {
            threadId: PARENT,
            session: "s1",
            agent: "planner",
            state: "closed",
            keepOpen: true,
          },
        }),
      ],
      cards: new Map([["session-3", card("s1", { state: "working", keepOpen: false })]]),
    });
    expect(m.open).toEqual([]);
    expect(m.closed.map((i) => [i.session, i.state, i.keepOpen])).toEqual([["s1", "closed", true]]);
  });

  it("keeps what only the card knows (the round and how it ended) on the merged subagent", () => {
    const m = merge({
      rows: [row(A, "s1")],
      cards: new Map([["session-3", card("s1", { state: "idle", round: 2, report: "failed" })]]),
    });
    expect(m.open[0]).toMatchObject({ round: 2, report: "failed" });
  });

  it("says a subagent waits for the person when its thread does, and not from a card's older word", () => {
    const m = merge({
      rows: [row(A, "s1", { state: "blocked" }), row(B, "s2", { state: "running" })],
      cards: new Map([
        ["session-3", card("s1", { report: "done" })],
        ["session-4", card("s2", { report: "blocked" })],
      ]),
    });
    expect(m.open.map((i) => i.report)).toEqual(["blocked", undefined]);
  });

  it("puts the open ones first and folds the closed ones apart, each group in the rail's order", () => {
    const m = merge({
      rows: [
        row(A, "s1", { childOf: { threadId: PARENT, session: "s1", state: "closed" } }),
        row(B, "s2"),
        row(C, "s3", { childOf: { threadId: PARENT, session: "s3", state: "working" } }),
      ],
    });
    expect(m.open.map((i) => i.session)).toEqual(["s2", "s3"]);
    expect(m.closed.map((i) => i.session)).toEqual(["s1"]);
  });

  it("applies the answer of a close at once, as closing, and a closed answer as closed", () => {
    const rows = [row(A, "s1"), row(B, "s2")];
    const m = merge({
      rows,
      applied: new Map([
        [A, "closing"],
        [B, "closed"],
      ]),
    });
    expect(m.open.map((i) => [i.session, i.closing])).toEqual([["s1", true]]);
    expect(m.closed.map((i) => i.session)).toEqual(["s2"]);
  });

  it("counts the starts that wait, and lists them as pending, but not the ones that opened or failed", () => {
    const starts = new Map([
      ["r1", start("r1")],
      ["r2", start("r2", { state: "opened", threadId: A, session: "s1" })],
      ["r3", start("r3", { state: "failed", reason: "no" })],
    ]);
    const m = merge({ starts });
    expect(m.pending.map((p) => p.request)).toEqual(["r1"]);
    expect(startsWaiting(starts, ["r4", "r2", "r1"])).toBe(2);
  });

  it("counts the open subagents, closing ones included", () => {
    const m = merge({ rows: [row(A, "s1"), row(B, "s2")], applied: new Map([[A, "closing"]]) });
    expect(m.open).toHaveLength(2);
  });
});

describe("a close the person asked, until the record says otherwise", () => {
  const ASKED = Date.parse("2026-11-20T10:00:00Z");
  const before = "2026-11-20T09:59:00Z";
  const after = "2026-11-20T10:00:05Z";
  const entry = (state: "closing" | "closed", cancel = false): AppliedEntry => ({
    state,
    cancel,
    at: ASKED,
  });
  const live = (
    entries: Record<string, AppliedEntry>,
    over: Partial<Parameters<typeof liveCloses>[1]> = {},
  ) =>
    liveCloses(new Map(Object.entries(entries)), {
      thread: PARENT,
      rows: [],
      cards: new Map(),
      self: null,
      ...over,
    });

  it("keeps a close that nothing newer contradicts", () => {
    const kept = live(
      { [A]: entry("closing") },
      {
        rows: [row(A, "s1", { updatedAt: before })],
        cards: new Map([["session-1", card("s1", { at: before })]]),
      },
    );
    expect([...kept.keys()]).toEqual([A]);
  });

  for (const cancel of [false, true]) {
    it(`drops a closing (cancel: ${cancel}) once a newer card says the session again`, () => {
      const kept = live(
        { [A]: entry("closing", cancel) },
        { cards: new Map([["session-1", card("s1", { at: after })]]) },
      );
      expect(kept.size).toBe(0);
    });

    it(`drops a closing (cancel: ${cancel}) once a newer row says the session again`, () => {
      const kept = live(
        { [A]: entry("closing", cancel) },
        { rows: [row(A, "s1", { updatedAt: after })] },
      );
      expect(kept.size).toBe(0);
    });

    it(`drops a closing (cancel: ${cancel}) once the child's own newer card says it`, () => {
      const kept = live(
        { [A]: entry("closing", cancel) },
        { thread: A, self: card("s1", { threadId: undefined, at: after }) },
      );
      expect(kept.size).toBe(0);
    });
  }

  it("does not read another session's newer card against this close", () => {
    const kept = live(
      { [A]: entry("closing") },
      { cards: new Map([["session-2", card("s2", { threadId: B, at: after })]]) },
    );
    expect(kept.size).toBe(1);
  });
});

describe("a card newer than the row", () => {
  it("wins on the state, as the child's own page reads it", () => {
    const m = merge({
      rows: [row(A, "s1", { updatedAt: "2026-11-20T10:00:00Z" })],
      cards: new Map([["session-1", card("s1", { state: "working", at: "2026-11-20T10:00:05Z" })]]),
    });
    expect(m.open.map((i) => i.state)).toEqual(["working"]);
  });

  it("loses to a newer row", () => {
    const m = merge({
      rows: [row(A, "s1", { updatedAt: "2026-11-20T10:00:05Z" })],
      cards: new Map([["session-1", card("s1", { state: "working", at: "2026-11-20T10:00:00Z" })]]),
    });
    expect(m.open.map((i) => i.state)).toEqual(["idle"]);
  });
});
