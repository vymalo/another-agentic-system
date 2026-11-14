"use client";

import { useAuiState } from "@assistant-ui/react";
import { createContext, type ReactNode, useContext, useSyncExternalStore } from "react";
import { agentNamed } from "@/features/agents/lib/selection";
import type { ApiAgent } from "@/lib/api/types";
import { type Mention, parseMentions, segments } from "../lib/mentions";
import type { MentionsStore } from "../lib/store";

type Known = { store: MentionsStore | null; agents: readonly ApiAgent[] };
const Context = createContext<Known>({ store: null, agents: [] });

/** What a bubble needs to draw mentions: the ones this page sent (by message id) and the agents' names. */
export function MentionsProvider({ store, agents, children }: Known & { children: ReactNode }) {
  return <Context.Provider value={{ store, agents }}>{children}</Context.Provider>;
}

/**
 * The mentions of the user message being drawn, validated against its text: those the log carried
 * (`metadata.custom.mentions`, a reload or another tab), else those this page sent with it (the
 * stream does not echo a message to the run that sent it).
 */
export function useMessageMentions(text: string): Mention[] {
  const id = useAuiState((s) => s.message.id);
  const custom = useAuiState((s) => s.message.metadata?.custom?.mentions);
  const source = useMentionSource(id, custom);
  return source ? parseMentions(source, text) : [];
}

/**
 * Whether the message with this `id` mentions an agent: what the log carried (`custom`, its
 * `metadata.custom.mentions`) or what this page sent with it. For a message other than the one
 * being drawn (the one a turn answers).
 */
export function useMentionsAny(id: string | undefined, custom: unknown): boolean {
  const source = useMentionSource(id, custom);
  return Array.isArray(source) && source.length > 0;
}

function useMentionSource(id: string | undefined, custom: unknown) {
  const { store } = useContext(Context);
  useSyncExternalStore(
    store?.subscribe ?? NO_SUBSCRIBE,
    store?.getVersion ?? ZERO,
    store?.getVersion ?? ZERO,
  );
  if (Array.isArray(custom)) return custom;
  return id === undefined ? undefined : store?.sentOf(id);
}

const NO_SUBSCRIBE = () => () => {};
const ZERO = () => 0;

/**
 * The words of a message that mentions agents: plain text with each mention drawn as a chip (the
 * agent's name on hover and to a screen reader), the rest as written, line breaks kept. A message
 * that mentions nobody keeps its Markdown; one that does is drawn as text, because the references
 * are offsets into the words as typed and Markdown would move them.
 */
export function MentionedText({ text, mentions }: { text: string; mentions: readonly Mention[] }) {
  const { agents } = useContext(Context);
  return (
    <p data-slot="mentioned-text" className="whitespace-pre-wrap">
      {segments(text, mentions).map((part, i) => {
        if (!("mention" in part)) return part.text;
        const m = part.mention;
        const name = agentNamed(agents, m.agentId)?.name;
        return (
          <span
            // biome-ignore lint/suspicious/noArrayIndexKey: the segments are the text, in order
            key={i}
            data-slot="mention"
            data-agent={m.agentId}
            title={name ? `${name} (${m.agentId})` : m.agentId}
            className="rounded-md bg-background/70 px-1 font-medium text-foreground"
          >
            {part.text}
          </span>
        );
      })}
    </p>
  );
}
