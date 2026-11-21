import { describe, expect, it } from "vitest";
import type { ApiThread } from "@/lib/api/types";
import type { SessionCard } from "./cards";
import { childView } from "./child-view";

const ID = "00000000-0000-4000-8000-000000000002";
const PARENT = "00000000-0000-4000-8000-000000000001";

const thread = (over: Record<string, unknown> = {}, updatedAt = "2026-11-20T10:00:00Z") =>
  ({
    id: ID,
    updatedAt,
    childOf: { threadId: PARENT, session: "s1", agent: "planner", state: "idle", keepOpen: true },
    ...over,
  }) as unknown as ApiThread;

const self = (over: Partial<SessionCard> = {}): SessionCard => ({
  session: "s1",
  agent: "planner",
  description: "",
  keepOpen: true,
  state: "working",
  closing: false,
  closeAsked: false,
  at: "2026-11-20T10:00:05Z",
  ...over,
});

describe("childView", () => {
  it("is null for a thread that is nobody's helper", () => {
    expect(
      childView({ thread: { id: ID } as unknown as ApiThread, threadId: ID, self: null }),
    ).toBe(null);
    expect(childView({ thread: null, threadId: ID, self: null })).toBe(null);
  });

  it("reads the resource's childOf", () => {
    const v = childView({ thread: thread(), threadId: ID, self: null });
    expect(v).toMatchObject({
      parent: PARENT,
      agent: "planner",
      state: "idle",
      keepOpen: true,
      phase: "open",
      threadId: ID,
    });
  });

  it("takes the live card when it is newer than the resource", () => {
    const v = childView({ thread: thread(), threadId: ID, self: self({ state: "working" }) });
    expect(v?.state).toBe("working");
    expect(v?.parent).toBe(PARENT);
  });

  it("keeps the resource when the card is older", () => {
    const v = childView({
      thread: thread({}, "2026-11-20T11:00:00Z"),
      threadId: ID,
      self: self({ state: "working", at: "2026-11-20T10:00:00Z" }),
    });
    expect(v?.state).toBe("idle");
  });

  it("knows a child from its card alone, before the resource is read", () => {
    const v = childView({ thread: null, threadId: ID, self: self({ closing: true }) });
    expect(v).toMatchObject({ phase: "closing", agent: "planner" });
    expect(v?.parent).toBeUndefined();
  });

  it("is closing while a close waits or is asked, and closed once closed", () => {
    const closing = thread({
      childOf: { threadId: PARENT, session: "s1", state: "working", closeAsked: true },
    });
    expect(childView({ thread: closing, threadId: ID, self: null })?.phase).toBe("closing");
    const closed = thread({ childOf: { threadId: PARENT, session: "s1", state: "closed" } });
    expect(childView({ thread: closed, threadId: ID, self: null })?.phase).toBe("closed");
  });

  it("applies a close the person just asked, before anything says it", () => {
    expect(
      childView({ thread: thread(), threadId: ID, self: null, applied: "closing" })?.phase,
    ).toBe("closing");
    expect(
      childView({ thread: thread(), threadId: ID, self: null, applied: "closed" }),
    ).toMatchObject({ phase: "closed", state: "closed" });
  });

  it("a link needs a UUID: a parent that is not one is no parent", () => {
    const v = childView({
      thread: thread({ childOf: { threadId: "../x", session: "s1", state: "idle" } }),
      threadId: ID,
      self: null,
    });
    expect(v?.parent).toBeUndefined();
  });
});

describe("childView, further decisions", () => {
  it("says no report of the live card when the resource is the newer", () => {
    const v = childView({
      thread: thread({}, "2026-11-20T11:00:00Z"),
      threadId: ID,
      self: self({ report: "failed", at: "2026-11-20T10:00:00Z" }),
    });
    expect(v?.report).toBeUndefined();
    expect(
      childView({ thread: thread(), threadId: ID, self: self({ report: "failed" }) })?.report,
    ).toBe("failed");
  });

  it("keeps the helper's name from the resource when the live card has none", () => {
    const v = childView({ thread: thread(), threadId: ID, self: self({ agent: "" }) });
    expect(v?.agent).toBe("planner");
  });
});
