/**
 * Contract drift guards. Compiled by `tsc --noEmit`, never imported.
 *
 * Each `@ts-expect-error` marks a deliberate mismatch with `docs/api/chat-api.yaml` that MUST be
 * a type error. If the contract changes so that a line stops erroring, the unused directive
 * fails the typecheck and someone has to look at what the contract now allows.
 */
import { api } from "./client";
import type { ApiThread, ThreadState } from "./types";

// @ts-expect-error "running" is not a ThreadState
export const badState: ThreadState = "running";

// @ts-expect-error Thread requires lastSeq
export const badThread: ApiThread = {
  id: "x",
  title: "t",
  target: { agentId: "a" },
  state: "done",
  createdAt: "",
  updatedAt: "",
};

export async function badCalls() {
  // @ts-expect-error NewThread requires target
  await api.POST("/api/threads", { body: { text: "hi" } });
  // @ts-expect-error unknown path
  await api.GET("/api/thread");
  await api.POST("/api/threads/{threadId}/messages", {
    params: { path: { threadId: "x" } },
    // @ts-expect-error NewMessage requires text
    body: {},
  });
}
