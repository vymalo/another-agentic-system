"use client";

import { useAui } from "@assistant-ui/react";
import { RotateCcwIcon } from "lucide-react";
import type { RefObject } from "react";
import { PandaMark } from "@/components/brand/panda-mark";
import { InlineStatus } from "@/components/inline-status";
import type { AgentsView } from "@/features/agents/hooks/use-agents";
import { selectedAgent } from "@/features/agents/lib/selection";
import type { Selection } from "@/features/chat/hooks/use-chat-runtime";

type Props = {
  agents: AgentsView;
  selection: Selection;
};

/** Why there is no agent to talk to: loading failed (with Retry), or none is configured. */
export function AgentsProblem({ agents }: { agents: AgentsView }) {
  const { agents: list, loading, error, retry } = agents;
  if (list.length > 0 || loading) return null;
  if (error) {
    return (
      <InlineStatus
        tone="error"
        role="alert"
        action={{ label: "Retry", icon: RotateCcwIcon, onClick: retry }}
      >
        Could not load agents: {error}
      </InlineStatus>
    );
  }
  return <InlineStatus>No agents are configured.</InlineStatus>;
}

/** The greeting of a new chat: the panda, a line of welcome and what the chosen agent does. */
export function NewChatGreeting({ agents, selection }: Props) {
  const agent = selectedAgent(agents.agents, selection);
  return (
    <div className="flex flex-col items-center gap-4 text-center">
      <PandaMark size={96} className="size-[72px] sm:size-24" />
      <h1 className="text-[1.75rem] leading-tight font-medium tracking-tight text-balance sm:text-[2rem]">
        What should we get done?
      </h1>
      <p className="max-w-md text-[0.9375rem] text-muted-foreground text-balance">
        {agent?.description ?? "Describe a task, and an agent takes it from there."}
      </p>
    </div>
  );
}

const SUGGESTIONS = [
  "Fix the failing login test",
  "Add input validation to the signup form",
  "Write tests for the payment module",
  "Explain how the auth flow works",
] as const;

/** Starting points: a chip puts its text in the box (it does not send it). */
export function Suggestions({ inputRef }: { inputRef: RefObject<HTMLTextAreaElement | null> }) {
  const aui = useAui();
  return (
    <ul aria-label="Suggestions" className="flex flex-wrap justify-center gap-2">
      {SUGGESTIONS.map((text) => (
        <li key={text}>
          <button
            type="button"
            onClick={() => {
              aui.composer().setText(text);
              inputRef.current?.focus();
            }}
            className="h-9 cursor-pointer rounded-full border bg-background px-3.5 text-[0.8125rem] text-foreground/85 transition-colors hover:bg-muted hover:text-foreground"
          >
            {text}
          </button>
        </li>
      ))}
    </ul>
  );
}
