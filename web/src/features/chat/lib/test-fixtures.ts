import type { ApiEvent } from "@/lib/api/types";

export const THREAD_ID = "11111111-1111-4111-8111-111111111111";
const AGENT = { type: "agent", name: "coder", revision: "coder-r47" } as const;
const USER = { type: "user", name: "dev@example.com" } as const;

let clock = 0;
const at = () => new Date(Date.UTC(2026, 8, 29, 9, 0, clock++)).toISOString();

export const userMessage = (seq: number, text = "Implement it"): ApiEvent => ({
  seq,
  threadId: THREAD_ID,
  at: at(),
  kind: "user_message",
  actor: USER,
  data: { text },
});

export const agentMessage = (
  seq: number,
  messageId: string,
  text: string,
  final: boolean,
): ApiEvent => ({
  seq,
  threadId: THREAD_ID,
  at: at(),
  kind: "agent_message",
  actor: AGENT,
  data: { text, messageId, final },
});

export const status = (seq: number, s: string, detail?: string): ApiEvent => ({
  seq,
  threadId: THREAD_ID,
  at: at(),
  kind: "agent_status",
  actor: AGENT,
  data: { status: s, ...(detail ? { detail } : {}) },
});

export const artifact = (seq: number, uri: string): ApiEvent => ({
  seq,
  threadId: THREAD_ID,
  at: at(),
  kind: "artifact",
  actor: AGENT,
  data: { name: "pull-request", mimeType: "text/uri-list", uri },
});

export const threadState = (seq: number, state: string): ApiEvent => ({
  seq,
  threadId: THREAD_ID,
  at: at(),
  kind: "thread_state",
  actor: { type: "system", name: "orchestrator" },
  data: { state } as ApiEvent["data"],
});

export const errorEvent = (seq: number, message: string, retryable = false): ApiEvent => ({
  seq,
  threadId: THREAD_ID,
  at: at(),
  kind: "error",
  actor: AGENT,
  data: { message, retryable },
});
