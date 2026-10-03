"use client";

import { InlineStatus } from "@/components/inline-status";
import type { AgentCapabilities } from "@/features/agents/hooks/use-agent-capabilities";
import { THREAD_TOOLS_URI } from "@/features/tools/lib/servers";
import { MENTIONS_URI } from "../lib/mentions";

/**
 * Said before the person sends (ADR 0026, the extension is optional: ADR 0008): what the agent that
 * reads the message can do with the agents it mentions, from its card as it is now.
 *
 * - no `mentions/v1`: it is not told who was mentioned; the labels reach it as the words they are;
 * - `mentions/v1` but no `thread-tools/v1`: it is told, and has no way to ask them;
 * - a card that could not be read: "Could not check", never "can" (fail closed), like the tools line.
 *
 * Nothing while nobody is mentioned or the card has not been read yet. Send is never disabled: a
 * mention is still the person's words.
 */
export function MentionsWarning({
  mentioned,
  capabilities,
  agentName,
}: {
  /** How many agents the message in the box mentions. */
  mentioned: number;
  capabilities: AgentCapabilities;
  agentName: string;
}) {
  if (mentioned === 0) return null;
  if (capabilities.status === "unreadable") {
    return (
      <InlineStatus tone="warning" role="status">
        <span data-slot="mentions-warning">
          Could not check whether {agentName} can work with the agents you mentioned.
        </span>
      </InlineStatus>
    );
  }
  if (capabilities.status !== "ready") return null;
  if (!capabilities.supports(MENTIONS_URI)) {
    return (
      <InlineStatus tone="warning" role="status">
        <span data-slot="mentions-warning">
          {agentName} does not use mentions, so it will not be told who you mentioned. The names
          stay in your message as text.
        </span>
      </InlineStatus>
    );
  }
  if (!capabilities.supports(THREAD_TOOLS_URI)) {
    return (
      <InlineStatus tone="warning" role="status">
        <span data-slot="mentions-warning">
          {agentName} will be told who you mentioned, but it cannot ask other agents.
        </span>
      </InlineStatus>
    );
  }
  return null;
}
