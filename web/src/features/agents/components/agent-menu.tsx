"use client";

import { ChevronDownIcon } from "lucide-react";
import Link from "next/link";
import type { ReactNode } from "react";
import { AgentAvatar } from "@/components/brand/agent-avatar";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuGroup,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Skeleton } from "@/components/ui/skeleton";
import type { AgentsView } from "@/features/agents/hooks/use-agents";
import { newChatWith, selectedAgent } from "@/features/agents/lib/selection";
import type { Selection } from "@/features/chat/hooks/use-chat-runtime";
import type { ApiAgent } from "@/lib/api/types";
import { cn } from "@/lib/utils";

type Common = {
  agents: AgentsView;
  /**
   * A line under the lists, inside the menu: the slot for what is true of all agents at once.
   * Plan 05 PR 5 puts its "The agent registry is unreachable; showing the configured agents only."
   * (`InlineStatus tone="warning"`) here; a failed refresh of the list is drawn above it.
   */
  notice?: ReactNode;
  className?: string;
};

export type AgentMenuProps = Common &
  (
    | {
        /** A new chat: the choice is the person's, and it is what the first message goes to. */
        mode: "new";
        value: Selection;
        onChange: (next: Selection) => void;
      }
    | {
        /**
         * An existing thread: a thread has one agent. The menu shows it, and offers the others as
         * a new chat. Changing the agent of a thread is a fork (plan 08, F-series), not built yet.
         */
        mode: "thread";
        value: Selection;
      }
  );

const releaseOf = (agent: ApiAgent | undefined, value: Selection): string | null =>
  agent?.releases ? (value.release ?? agent.releases.defaultChannel) : null;

/** An agent in the menu: its mark, its name and, in one line, what it does (from its live card). */
function AgentLabel({
  agent,
  release,
}: {
  agent: Pick<ApiAgent, "id" | "name" | "description">;
  release?: string | null;
}) {
  return (
    <>
      <AgentAvatar agentId={agent.id} name={agent.name} size={24} className="mt-0.5" />
      <span className="flex min-w-0 flex-col gap-0.5">
        <span className="text-sm leading-5 font-medium">
          {agent.name}
          {release ? <span className="font-normal text-muted-foreground"> · {release}</span> : null}
        </span>
        {agent.description ? (
          <span className="line-clamp-1 text-xs leading-4 text-muted-foreground">
            {agent.description}
          </span>
        ) : null}
      </span>
    </>
  );
}

/** What the person is told when the list could not be refreshed: it stays as it was. */
function Problem({ message, onRetry }: { message: string; onRetry: () => void }) {
  return (
    <>
      <DropdownMenuSeparator />
      <p role="status" className="px-2.5 py-1.5 text-xs text-destructive">
        Could not refresh the agents: {message}
      </p>
      <DropdownMenuItem onSelect={onRetry}>Retry</DropdownMenuItem>
    </>
  );
}

/**
 * The agent picker, in the top bar like a model picker: a button with the agent's name ("Coder ⌄")
 * that opens a menu of the agents (name, one line of what it does, a check on the chosen one) and,
 * when the chosen agent offers releases, its release as a second group of the same menu (inline, so
 * it works from a phone and from the keyboard). The list is read again each time the menu opens:
 * releases are the agent's card right now, never cached (ADR 0008).
 *
 * Radix's menu is the keyboard contract: arrows move, Enter or Space chooses, Escape closes and
 * focus goes back to the button, letters jump to an agent.
 */
export function AgentMenu(props: AgentMenuProps) {
  const { agents, value, notice, className } = props;
  const { agents: list, loading, error, retry } = agents;
  const isNew = props.mode === "new";

  // a new chat falls back to the first agent; an existing thread names its own, listed or not
  const current = isNew ? selectedAgent(list, value) : list.find((a) => a.id === value.agentId);
  const agentId = current?.id ?? value.agentId;

  if (list.length === 0 && isNew) {
    if (loading) {
      return (
        <span role="status" className={cn("inline-flex px-3", className)}>
          <span className="sr-only">Loading agents…</span>
          <Skeleton aria-hidden="true" className="h-5 w-24" />
        </span>
      );
    }
    if (!error) {
      return (
        <Button
          type="button"
          variant="ghost"
          disabled
          className={cn("h-9 rounded-full px-3 text-base font-medium", className)}
        >
          No agents
        </Button>
      );
    }
  }
  if (agentId === null && !(list.length === 0 && isNew)) {
    return (
      <span role="status" className={cn("inline-flex px-3", className)}>
        <span className="sr-only">Loading agent…</span>
        <Skeleton aria-hidden="true" className="h-5 w-24" />
      </span>
    );
  }

  const name = current?.name ?? agentId ?? "Agents unavailable";
  const release = releaseOf(current, value);
  const releases = isNew ? current?.releases : undefined;
  const others = isNew ? [] : list.filter((a) => a.id !== agentId);

  return (
    // not modal: a picker in the top bar does not need the page behind it hidden from a screen
    // reader (axe reports a modal menu's aria-hidden siblings as focusable), and a click or a Tab
    // outside closes it all the same
    <DropdownMenu modal={false} onOpenChange={(open) => open && retry()}>
      <DropdownMenuTrigger asChild>
        <Button
          type="button"
          variant="ghost"
          data-slot="agent-menu-trigger"
          className={cn(
            "h-9 max-w-[min(100%,16rem)] min-w-0 gap-1.5 rounded-full ps-3 pe-2.5 text-base font-medium text-foreground",
            className,
          )}
        >
          {/* the words of the name come first, so a screen reader hears "Agent: Coder · stable" */}
          <span className="sr-only">Agent: </span>
          <span className="min-w-0 truncate">
            <span className={current ? undefined : "capitalize"}>{name}</span>
            {release ? (
              <span className="font-normal text-muted-foreground"> · {release}</span>
            ) : null}
          </span>
          <ChevronDownIcon aria-hidden="true" className="size-4 text-muted-foreground" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent
        align="start"
        className="max-h-(--radix-dropdown-menu-content-available-height) w-[min(23.75rem,calc(100vw-1rem))] overflow-y-auto p-1.5"
      >
        {props.mode === "new" ? (
          list.length > 0 ? (
            <>
              <DropdownMenuLabel aria-hidden="true">Agents</DropdownMenuLabel>
              <DropdownMenuRadioGroup
                aria-label="Agents"
                value={agentId ?? ""}
                onValueChange={(id) => props.onChange({ agentId: id, release: null })}
              >
                {list.map((a) => (
                  <DropdownMenuRadioItem key={a.id} value={a.id} textValue={a.name}>
                    <AgentLabel agent={a} />
                  </DropdownMenuRadioItem>
                ))}
              </DropdownMenuRadioGroup>
              {releases && current ? (
                <>
                  <DropdownMenuSeparator />
                  <DropdownMenuLabel aria-hidden="true">Release</DropdownMenuLabel>
                  <DropdownMenuRadioGroup
                    aria-label="Release"
                    value={release ?? ""}
                    onValueChange={(next) => props.onChange({ agentId: current.id, release: next })}
                  >
                    {Object.entries(releases.channels).map(([channel, revision]) => (
                      <DropdownMenuRadioItem
                        key={`channel:${channel}`}
                        value={channel}
                        textValue={channel}
                        className="items-center py-1.5"
                      >
                        <span className="text-sm">
                          {channel}
                          <span className="text-muted-foreground"> — {revision}</span>
                        </span>
                      </DropdownMenuRadioItem>
                    ))}
                    {releases.revisions?.length ? (
                      <DropdownMenuLabel aria-hidden="true" className="pt-2">
                        Revisions
                      </DropdownMenuLabel>
                    ) : null}
                    {(releases.revisions ?? []).map((revision) => (
                      <DropdownMenuRadioItem
                        key={`revision:${revision}`}
                        value={revision}
                        textValue={revision}
                        className="items-center py-1.5"
                      >
                        <span className="text-sm">{revision}</span>
                      </DropdownMenuRadioItem>
                    ))}
                  </DropdownMenuRadioGroup>
                </>
              ) : null}
            </>
          ) : (
            <DropdownMenuItem onSelect={retry}>Retry loading the agents</DropdownMenuItem>
          )
        ) : (
          <>
            <DropdownMenuLabel aria-hidden="true">This chat</DropdownMenuLabel>
            <DropdownMenuRadioGroup aria-label="This chat" value={agentId ?? ""}>
              <DropdownMenuRadioItem value={agentId ?? ""} textValue={name}>
                <AgentLabel
                  agent={current ?? { id: agentId ?? "", name }}
                  release={value.release}
                />
              </DropdownMenuRadioItem>
            </DropdownMenuRadioGroup>
            {others.length > 0 ? (
              <>
                <DropdownMenuSeparator />
                <DropdownMenuLabel aria-hidden="true">Start a new chat with</DropdownMenuLabel>
                <DropdownMenuGroup aria-label="Start a new chat with">
                  {others.map((a) => (
                    <DropdownMenuItem
                      key={a.id}
                      asChild
                      textValue={a.name}
                      className="items-start gap-3 py-2 [&_svg]:text-current"
                    >
                      <Link href={newChatWith(a.id)} className="text-foreground no-underline">
                        <AgentLabel agent={a} />
                      </Link>
                    </DropdownMenuItem>
                  ))}
                </DropdownMenuGroup>
              </>
            ) : null}
          </>
        )}
        {error && list.length > 0 ? <Problem message={error} onRetry={retry} /> : null}
        {notice}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
