"use client";

import { TextMessagePartProvider } from "@assistant-ui/react";
import { createContext, type FC, type ReactNode, useContext, useSyncExternalStore } from "react";
import { MarkdownText } from "@/components/assistant-ui/elements/markdown-text";
import type { Draft } from "@/features/chat/lib/agui/live-drafts";
import type { ThreadAgent } from "@/features/chat/lib/agui/thread-agent";

/*
 * The words of a reply that is still being written (docs/api/agui.md "Live text"). They are not in
 * the runtime's transcript, which is the log and has the reply only when it is final: the agent
 * keeps them as drafts (`live-drafts.ts`) and the turn draws them after its parts, in the same
 * place and the same type as the final words, with a caret. When the log's message arrives it is
 * drawn by the runtime as any reply is, and the draft draws nothing.
 *
 * The drafts come through a context of their own, fed by a provider that subscribes by itself, so a
 * piece of text (several a second) renders the one turn that draws it, not the whole chat.
 */

const Context = createContext<readonly Draft[]>([]);

export function LiveDraftsProvider({
  agent,
  children,
}: {
  agent: ThreadAgent;
  children: ReactNode;
}) {
  const drafts = useSyncExternalStore(agent.onDraftsChange, agent.getDrafts, agent.getDrafts);
  return <Context.Provider value={drafts}>{children}</Context.Provider>;
}

export const useLiveDrafts = (): readonly Draft[] => useContext(Context);

/**
 * One draft: prose like the agent's other words (`data-slot="agent-draft"`, where the finished
 * reply is `agent-message`), `aria-busy` and silent for assistive technology until it is final (the
 * log's message is announced once, whole, as every reply is), and a caret that blinks unless the
 * person asked for less motion (`.draft-caret`, globals.css).
 */
export const LiveDraft: FC<{ id: string; text: string }> = ({ id, text }) => (
  <div
    data-slot="agent-draft"
    data-role="assistant"
    data-message-id={id}
    aria-busy="true"
    aria-live="off"
    className="min-w-0 [overflow-wrap:anywhere]"
  >
    <div className="draft-caret text-[0.9375rem] leading-7 text-foreground">
      <TextMessagePartProvider text={text} isRunning>
        <MarkdownText />
      </TextMessagePartProvider>
    </div>
  </div>
);
