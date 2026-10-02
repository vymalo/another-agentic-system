import type { ApiAgent, ApiMe } from "@/lib/api/types";

/*
 * What `GET /api/me` says a person may do, as questions a screen asks. Nothing here is a check:
 * the orchestrator enforces every request whatever the web shows (ADR 0033), so a screen that
 * hides what it should not have shown costs nothing, and one that shows what the server then
 * refuses says so with the server's words (the problem's `detail`, where the action was).
 */

type Permission = ApiMe["permissions"][number]["permission"];
type Scope = NonNullable<ApiMe["permissions"][number]["scope"]>;

/** The scope a permission over threads has for this person; null when no role of theirs holds it. */
export function scopeOf(
  me: ApiMe,
  permission: "thread.read" | "thread.write" | "artifact.read",
): Scope | null {
  return me.permissions.find((p) => p.permission === permission)?.scope ?? null;
}

export const holds = (me: ApiMe, permission: Permission): boolean =>
  me.permissions.some((p) => p.permission === permission);

/** Nothing is granted at all: every route but `getMe` answers 403 `no_access`. */
export const hasNoAccess = (me: ApiMe): boolean => me.permissions.length === 0;

/**
 * An administrator reads every thread (`admin` and a `thread.read` of scope `any`): the person the
 * "All threads" list of the sidebar is for, which `GET /api/threads?owner=*` serves.
 */
export const isAdmin = (me: ApiMe): boolean =>
  holds(me, "admin") && scopeOf(me, "thread.read") === "any";

const names = (list: readonly string[], id: string): boolean =>
  list.includes("*") || list.includes(id);

/** Whether `agent.invoke` covers the agent: it can be started, and written to. */
export const mayInvoke = (me: ApiMe, agentId: string): boolean => names(me.agents.invoke, agentId);

/**
 * The agents a person may start a chat with: the listed ones that `agent.invoke` covers (the list
 * is already the ones `agent.read` covers). `keep` stays in whatever the roles say: a thread names
 * its own agent, and the menu says its name.
 */
export function invokable(list: ApiAgent[], me: ApiMe | null, keep?: string | null): ApiAgent[] {
  if (!me) return list;
  return list.filter((a) => a.id === keep || mayInvoke(me, a.id));
}

/** What a person may do in the open thread, and why not when they may not. */
export type ThreadAccess = { readOnly: false } | { readOnly: true; reason: string };

export const WRITABLE: ThreadAccess = { readOnly: false };

const sameUser = (a: string, b: string): boolean =>
  a.trim().toLowerCase() === b.trim().toLowerCase();

/**
 * Whether the person may act on a thread (write in it, answer its questions, rename, fork, cancel).
 * Reading is not acting: a thread is read-only for the person when `thread.write` is not theirs at
 * all, when its scope is `own` and the thread's `owner` is someone else (an administrator's view of
 * another's thread), or when `agent.invoke` does not cover its agent. `me` unknown (it could not be
 * read) or the thread not yet known is writable: the server decides, and says why when it refuses.
 */
export function threadAccess(
  me: ApiMe | null,
  thread: { owner: string; target: { agentId: string } } | null,
): ThreadAccess {
  if (!me || !thread) return WRITABLE;
  const write = scopeOf(me, "thread.write");
  if (write === null) {
    return { readOnly: true, reason: "Read only: your roles do not let you write in threads." };
  }
  if (write === "own" && !sameUser(thread.owner, me.user)) {
    return { readOnly: true, reason: `Read only: this is ${thread.owner}’s thread.` };
  }
  if (!mayInvoke(me, thread.target.agentId)) {
    return {
      readOnly: true,
      reason: `Read only: your roles do not let you use the ${thread.target.agentId} agent.`,
    };
  }
  return WRITABLE;
}

/** Whether the person may start a chat at all: `thread.write` and at least one agent to invoke. */
export function newChatAccess(me: ApiMe | null): ThreadAccess {
  if (!me) return WRITABLE;
  if (scopeOf(me, "thread.write") === null) {
    return { readOnly: true, reason: "Your roles do not let you start chats." };
  }
  if (me.agents.invoke.length === 0) {
    return { readOnly: true, reason: "Your roles do not let you use any agent." };
  }
  return WRITABLE;
}
