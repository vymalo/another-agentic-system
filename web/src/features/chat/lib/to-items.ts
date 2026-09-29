import type { ExternalStoreMessageConverter } from "@assistant-ui/react";
import type { AgentStatus, ApiActor, ArtifactData, TypedEvent } from "@/lib/api/types";
import { detectPullRequest, type PullRequestLink } from "./artifact";

type Common = { id: string; seq: number; at: string; actor: ApiActor };

/** One entry per visible event. Each carries its own actor, so no grouping is needed. */
export type ChatItem = Common &
  (
    | { kind: "user"; text: string }
    | { kind: "agent_text"; text: string; final: boolean }
    | { kind: "status"; status: AgentStatus; detail?: string }
    | { kind: "artifact"; artifact: ArtifactData }
    | { kind: "error"; message: string; retryable: boolean }
  );

/** Data carried by the `data-*` message parts, read by the part renderers. */
export type StatusPartData = { status: AgentStatus; detail?: string; actor: ApiActor };
export type ArtifactPartData = ArtifactData & { actor: ApiActor; pr?: PullRequestLink };
export type ErrorPartData = { message: string; retryable: boolean; actor: ApiActor };

/** Events (already filtered by `visibleEvents`) to chat items. Ids are stable across re-renders. */
export function toItems(events: readonly TypedEvent[]): ChatItem[] {
  const items: ChatItem[] = [];
  for (const e of events) {
    const base = { seq: e.seq, at: e.at, actor: e.actor };
    switch (e.kind) {
      case "user_message":
        items.push({ ...base, id: `evt-${e.seq}`, kind: "user", text: e.data.text });
        break;
      case "agent_message":
        items.push({
          ...base,
          id: `msg-${e.data.messageId}`,
          kind: "agent_text",
          text: e.data.text,
          final: e.data.final,
        });
        break;
      case "agent_status":
        items.push({
          ...base,
          id: `evt-${e.seq}`,
          kind: "status",
          status: e.data.status,
          ...(e.data.detail !== undefined ? { detail: e.data.detail } : {}),
        });
        break;
      case "artifact":
        items.push({ ...base, id: `evt-${e.seq}`, kind: "artifact", artifact: e.data });
        break;
      case "error":
        items.push({
          ...base,
          id: `evt-${e.seq}`,
          kind: "error",
          message: e.data.message,
          retryable: e.data.retryable,
        });
        break;
      case "thread_state":
        break; // shown as the header badge, never in the transcript
    }
  }
  return items;
}

const COMPLETE = { type: "complete", reason: "stop" } as const;

export const convertMessage: ExternalStoreMessageConverter<ChatItem> = (item) => {
  const common = {
    id: item.id,
    createdAt: new Date(item.at),
    metadata: { custom: { actor: item.actor } },
  };
  switch (item.kind) {
    case "user":
      return { ...common, role: "user", content: [{ type: "text", text: item.text }] };
    case "agent_text":
      return {
        ...common,
        role: "assistant",
        content: [{ type: "text", text: item.text }],
        status: item.final ? COMPLETE : { type: "running" },
      };
    case "status": {
      const data: StatusPartData = {
        status: item.status,
        ...(item.detail !== undefined ? { detail: item.detail } : {}),
        actor: item.actor,
      };
      return {
        ...common,
        role: "assistant",
        content: [{ type: "data-status", data }],
        status: COMPLETE,
      };
    }
    case "artifact": {
      const pr = detectPullRequest(item.artifact.uri);
      const data: ArtifactPartData = {
        ...item.artifact,
        actor: item.actor,
        ...(pr ? { pr } : {}),
      };
      return {
        ...common,
        role: "assistant",
        content: [{ type: "data-artifact", data }],
        status: COMPLETE,
      };
    }
    case "error": {
      const data: ErrorPartData = {
        message: item.message,
        retryable: item.retryable,
        actor: item.actor,
      };
      return {
        ...common,
        role: "assistant",
        content: [{ type: "data-error", data }],
        status: COMPLETE,
      };
    }
  }
};
