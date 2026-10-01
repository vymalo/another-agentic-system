"use client";

import { useAui } from "@assistant-ui/react";
import { ChevronDownIcon } from "lucide-react";
import type { ReactNode, RefObject } from "react";
import { BrandMark } from "@/components/brand-mark";
import { InlineStatus } from "@/components/inline-status";
import { Skeleton } from "@/components/ui/skeleton";
import type { AgentsView } from "@/features/agents/hooks/use-agents";
import type { Selection } from "@/features/chat/hooks/use-chat-runtime";
import { cn } from "@/lib/utils";

type Props = {
  agents: AgentsView;
  selection: Selection;
  onSelect: (s: Selection) => void;
};

type Agent = AgentsView["agents"][number];
type Releases = NonNullable<Agent["releases"]>;

function isKnown(releases: Releases, value: string) {
  return value in releases.channels || (releases.revisions ?? []).includes(value);
}

/** The agent the selection names, else the first. */
const selected = (list: readonly Agent[], selection: Selection): Agent | undefined =>
  list.find((a) => a.id === selection.agentId) ?? list[0];

/**
 * A native select dressed as a pill: the best picker on a phone, it groups revisions in an
 * `<optgroup>`, and its label is for screen readers (the pill says what it is).
 */
function PillSelect({
  id,
  label,
  value,
  onChange,
  children,
  className,
}: {
  id: string;
  label: string;
  value: string;
  onChange: (value: string) => void;
  children: ReactNode;
  className?: string;
}) {
  return (
    <span className={cn("relative inline-flex min-w-0", className)}>
      <label htmlFor={id} className="sr-only">
        {label}
      </label>
      <select
        id={id}
        value={value}
        onChange={(e) => onChange(e.target.value)}
        className="h-8 max-w-full min-w-0 cursor-pointer appearance-none truncate rounded-full border bg-background ps-3 pe-8 text-[0.8125rem] font-medium text-foreground outline-none transition-colors hover:bg-muted focus-visible:ring-3 focus-visible:ring-ring/40"
      >
        {children}
      </select>
      <ChevronDownIcon
        aria-hidden="true"
        className="pointer-events-none absolute end-2.5 top-1/2 size-3.5 -translate-y-1/2 text-muted-foreground"
      />
    </span>
  );
}

/** The agent picker, plus a release picker only when the selected agent offers `releases`. */
export function AgentPicker({ agents, selection, onSelect }: Props) {
  const { agents: list, loading } = agents;
  if (loading && list.length === 0) {
    return (
      <span role="status" className="inline-flex">
        <span className="sr-only">Loading agents…</span>
        <Skeleton aria-hidden="true" className="h-8 w-28 rounded-full" />
      </span>
    );
  }
  if (list.length === 0) return null;
  const agent = selected(list, selection);
  const releases = agent?.releases;
  const release =
    releases && selection.release && isKnown(releases, selection.release)
      ? selection.release
      : releases?.defaultChannel;
  return (
    <>
      <PillSelect
        id="agent"
        label="Agent"
        value={agent?.id ?? ""}
        onChange={(agentId) => onSelect({ agentId, release: null })}
      >
        {list.map((a) => (
          <option key={a.id} value={a.id}>
            {a.name}
          </option>
        ))}
      </PillSelect>
      {releases ? (
        <PillSelect
          id="release"
          label="Release"
          value={release ?? ""}
          onChange={(value) => onSelect({ agentId: agent?.id ?? null, release: value })}
          className="max-sm:max-w-[45%]"
        >
          {Object.entries(releases.channels).map(([channel, revision]) => (
            <option key={channel} value={channel}>
              {channel} — {revision}
            </option>
          ))}
          {releases.revisions?.length ? (
            <optgroup label="Revisions">
              {releases.revisions.map((rev) => (
                <option key={rev} value={rev}>
                  {rev}
                </option>
              ))}
            </optgroup>
          ) : null}
        </PillSelect>
      ) : null}
    </>
  );
}

/** Why there is no agent to talk to: loading failed (with Retry), or none is configured. */
export function AgentsProblem({ agents }: { agents: AgentsView }) {
  const { agents: list, loading, error, retry } = agents;
  if (list.length > 0 || loading) return null;
  if (error) {
    return (
      <InlineStatus tone="error" role="alert" action={{ label: "Retry", onClick: retry }}>
        Could not load agents: {error}
      </InlineStatus>
    );
  }
  return <InlineStatus>No agents are configured.</InlineStatus>;
}

/** The greeting of a new chat: the mark, a two-line welcome and what the chosen agent does. */
export function NewChatGreeting({ agents, selection }: Omit<Props, "onSelect">) {
  const agent = selected(agents.agents, selection);
  return (
    <div className="flex flex-col items-center gap-4 text-center">
      <BrandMark className="size-11" />
      <h1 className="text-[1.75rem] leading-tight font-medium tracking-tight text-balance sm:text-[2rem]">
        What should we build today?
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
