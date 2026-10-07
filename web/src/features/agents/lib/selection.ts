import type { Selection } from "@/features/chat/hooks/use-chat-runtime";
import type { ApiAgent } from "@/lib/api/types";

type Releases = NonNullable<ApiAgent["releases"]>;

/** Whether `value` names a channel or a revision the agent offers right now. */
export function isKnownRelease(releases: Releases, value: string): boolean {
  return value in releases.channels || (releases.revisions ?? []).includes(value);
}

/** The listed agent that answers to `id`: the one with this id, else the one that lists it as an alias. */
export function agentNamed(list: readonly ApiAgent[], id: string): ApiAgent | undefined {
  return list.find((a) => a.id === id) ?? list.find((a) => a.aliases?.includes(id));
}

/**
 * The agent the selection names (by its id or an alias of it: a choice remembered before an agent
 * was renamed), else the first one (a stale choice must not send nowhere).
 */
export function selectedAgent(
  list: readonly ApiAgent[],
  selection: Selection,
): ApiAgent | undefined {
  return (selection.agentId === null ? undefined : agentNamed(list, selection.agentId)) ?? list[0];
}

/**
 * What a new chat sends to: the agent the selection names (else the first), and, only for an
 * agent that offers releases, the chosen one when it still exists (else its default channel).
 * Releases are read live (ADR 0008), so a choice made against an older list is checked again.
 */
export function effectiveSelection(list: readonly ApiAgent[], selection: Selection): Selection {
  const agent = selectedAgent(list, selection);
  if (!agent) return { agentId: selection.agentId, release: null };
  const releases = agent.releases;
  if (!releases) return { agentId: agent.id, release: null };
  const known = selection.release !== null && isKnownRelease(releases, selection.release);
  return { agentId: agent.id, release: known ? selection.release : releases.defaultChannel };
}

/**
 * The agent a link asked for (`/?agent=reviewer`, what "Start a new chat with Reviewer" opens):
 * its id when the list has it (a link that names an alias opens the agent it names), else nothing.
 */
export function requestedAgent(list: readonly ApiAgent[], search: string): string | null {
  const id = new URLSearchParams(search).get("agent");
  return (id !== null && agentNamed(list, id)?.id) || null;
}

/** The path of a new chat with an agent already chosen. */
export const newChatWith = (agentId: string): string => `/?agent=${encodeURIComponent(agentId)}`;
