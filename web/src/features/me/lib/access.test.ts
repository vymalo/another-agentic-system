import { describe, expect, it } from "vitest";
import type { ApiAgent, ApiMe } from "@/lib/api/types";
import {
  hasNoAccess,
  holds,
  invokable,
  mayInvoke,
  newChatAccess,
  scopeOf,
  threadAccess,
} from "./access";

const me = (over: Partial<ApiMe> = {}): ApiMe => ({
  user: "dev@example.com",
  roles: ["user"],
  permissions: [
    { permission: "agent.read" },
    { permission: "agent.invoke" },
    { permission: "thread.read", scope: "own" },
    { permission: "thread.write", scope: "own" },
    { permission: "artifact.read", scope: "own" },
  ],
  agents: { read: ["*"], invoke: ["*"] },
  ...over,
});
const admin = me({
  user: "admin@example.com",
  roles: ["admin"],
  permissions: [
    { permission: "agent.read" },
    { permission: "agent.invoke" },
    { permission: "thread.read", scope: "own" },
    { permission: "thread.write", scope: "own" },
    { permission: "artifact.read", scope: "own" },
    { permission: "admin" },
  ],
});
const nobody = me({ roles: [], permissions: [], agents: { read: [], invoke: [] } });
const thread = (agentId = "coder") => ({ target: { agentId } });

describe("what a person's permissions are", () => {
  it("the scope of a permission over threads, and none for a permission that is not held", () => {
    expect(scopeOf(me(), "thread.write")).toBe("own");
    expect(scopeOf(nobody, "thread.read")).toBeNull();
  });

  it("an administrator reaches no more threads than a user: admin is held, the scope is own (ADR 0039)", () => {
    expect(holds(admin, "admin")).toBe(true);
    expect(holds(me(), "admin")).toBe(false);
    for (const permission of ["thread.read", "thread.write", "artifact.read"] as const) {
      expect(scopeOf(admin, permission)).toBe("own");
    }
  });

  it("nothing granted is no access", () => {
    expect(hasNoAccess(nobody)).toBe(true);
    expect(hasNoAccess(me())).toBe(false);
  });
});

describe("agents", () => {
  const list = ["coder", "reviewer", "verifier"].map((id) => ({ id, name: id })) as ApiAgent[];
  it("`*` is every agent, and a list is those", () => {
    expect(mayInvoke(me(), "anything")).toBe(true);
    const some = me({ agents: { read: ["*"], invoke: ["reviewer"] } });
    expect(mayInvoke(some, "reviewer")).toBe(true);
    expect(mayInvoke(some, "coder")).toBe(false);
    expect(mayInvoke(me({ agents: { read: ["*"], invoke: [] } }), "reviewer")).toBe(false);
  });

  it("the picker lists the invokable agents, the thread's own agent whatever the roles say", () => {
    const some = me({ agents: { read: ["*"], invoke: ["reviewer"] } });
    expect(invokable(list, some).map((a) => a.id)).toEqual(["reviewer"]);
    expect(invokable(list, some, "coder").map((a) => a.id)).toEqual(["coder", "reviewer"]);
    // unknown identity: the list as it is, the server decides
    expect(invokable(list, null)).toBe(list);
  });
});

describe("threadAccess", () => {
  it("is writable when the roles allow it and the agent may be invoked", () => {
    expect(threadAccess(me(), thread())).toEqual({ readOnly: false });
    // the administrator is a user over their own threads, and nothing more
    expect(threadAccess(admin, thread())).toEqual({ readOnly: false });
  });

  it("no thread.write at all, and an agent that is not invokable, are read-only with their own words", () => {
    const viewer = me({ permissions: [{ permission: "thread.read", scope: "own" }] });
    expect(threadAccess(viewer, thread())).toEqual({
      readOnly: true,
      reason: "Read only: your roles do not let you write in threads.",
    });
    const some = me({ agents: { read: ["*"], invoke: ["reviewer"] } });
    expect(threadAccess(some, thread("coder"))).toEqual({
      readOnly: true,
      reason: "Read only: your roles do not let you use the coder agent.",
    });
    expect(threadAccess(some, thread("reviewer"))).toEqual({ readOnly: false });
  });

  it("an identity or a thread that is not known yet hides nothing", () => {
    expect(threadAccess(null, thread())).toEqual({ readOnly: false });
    expect(threadAccess(admin, null)).toEqual({ readOnly: false });
  });
});

describe("newChatAccess", () => {
  it("needs thread.write and an agent to invoke", () => {
    expect(newChatAccess(me())).toEqual({ readOnly: false });
    expect(newChatAccess(null)).toEqual({ readOnly: false });
    expect(
      newChatAccess(me({ permissions: [{ permission: "thread.read", scope: "own" }] })),
    ).toMatchObject({ readOnly: true, reason: "Your roles do not let you start chats." });
    expect(newChatAccess(me({ agents: { read: ["*"], invoke: [] } }))).toMatchObject({
      readOnly: true,
      reason: "Your roles do not let you use any agent.",
    });
  });
});
