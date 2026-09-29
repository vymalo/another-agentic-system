/**
 * Contract drift guards. Compiled by `tsc --noEmit`, never imported.
 *
 * Each `@ts-expect-error` marks a deliberate mismatch with `docs/api/chat-api.yaml` that MUST be
 * a type error. If the contract changes so that a line stops erroring, the unused directive
 * fails the typecheck and someone has to look at what the contract now allows.
 */
import { api } from "./client";
import type { components } from "./schema";
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

/** The AG-UI operations: bodies and documents are the vendored AG-UI 1.0 schema, by reference. */
type RunInput = components["schemas"]["AgUiRunAgentInput"];
type Capabilities = components["schemas"]["AgUiAgentCapabilities"];
type AgUiEvent = components["schemas"]["AgUiEvent"];

export const goodRunInput: RunInput = {
  threadId: "00000000-0000-7000-8000-000000000001",
  runId: "run-1",
  messages: [{ id: "m1", role: "user", content: "hi" }],
};

// @ts-expect-error RunAgentInput requires runId
export const badRunInput: RunInput = { threadId: "t", messages: [] };

export const goodCapabilities: Capabilities = { transport: { streaming: true, resumable: true } };

// @ts-expect-error "RUN_STARTED" needs threadId and runId
export const badEvent: AgUiEvent = { type: "RUN_STARTED" };

export async function badAgUiCalls() {
  await api.POST("/agui/agents/{agentId}", {
    params: { path: { agentId: "coder" } },
    body: goodRunInput,
    parseAs: "stream",
  });
  await api.POST("/agui/agents/{agentId}", {
    params: { path: { agentId: "coder" } },
    // @ts-expect-error RunAgentInput requires messages
    body: { threadId: "t", runId: "r" },
  });
  await api.GET("/agui/threads/{threadId}/connect", {
    params: { path: { threadId: "x" }, query: { mode: "run" }, header: { "Last-Event-ID": "4" } },
    parseAs: "stream",
  });
  await api.GET("/agui/threads/{threadId}/connect", {
    // @ts-expect-error mode is `run` or absent
    params: { path: { threadId: "x" }, query: { mode: "forever" } },
  });
  await api.GET("/agui/agents/{agentId}/capabilities", { params: { path: { agentId: "coder" } } });
  // @ts-expect-error the capabilities document has no POST
  await api.POST("/agui/agents/{agentId}/capabilities", {});
}
