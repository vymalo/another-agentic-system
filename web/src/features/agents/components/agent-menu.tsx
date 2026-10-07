"use client";

import { ChevronDownIcon } from "lucide-react";
import { type ReactNode, useRef, useState } from "react";
import { AgentAvatar } from "@/components/brand/agent-avatar";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Skeleton } from "@/components/ui/skeleton";
import { RegistryMenuNotice } from "@/features/agents/components/registry-notice";
import type { AgentsView } from "@/features/agents/hooks/use-agents";
import { agentNamed, selectedAgent } from "@/features/agents/lib/selection";
import type { Selection } from "@/features/chat/hooks/use-chat-runtime";
import type { ApiAgent } from "@/lib/api/types";
import { cn } from "@/lib/utils";

type Common = {
  agents: AgentsView;
  /**
   * A line under the lists, inside the menu: the slot for what is true of all agents at once. By
   * default it is the registry's "unreachable; showing the configured agents only"
   * (`RegistryMenuNotice`, nothing while every source answers); a failed refresh of the list is
   * drawn above it.
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
         * An existing thread: a thread has one agent, so choosing another agent (or another release)
         * is a fork, not a switch: the menu asks "Continue with … in a new chat?" and `onContinue`
         * copies the conversation into a new thread (ADR 0029); this chat stays as it is.
         */
        mode: "thread";
        value: Selection;
        /** Makes the fork and goes to it; resolves `false` when it could not be made (the page says why). */
        onContinue: (to: Selection) => Promise<boolean>;
        /**
         * Why the conversation cannot be continued now (the agent's turn is going on): the other
         * choices are disabled and the menu says so. Absent when it can.
         */
        blocked?: string;
      }
  );

const releaseOf = (agent: ApiAgent | undefined, value: Selection): string | null =>
  agent?.releases ? (value.release ?? agent.releases.defaultChannel) : null;

/** An agent in the menu: its mark, its name and, in one line, what it does (from its live card). */
function AgentLabel({
  agent,
  release,
}: {
  agent: Pick<ApiAgent, "id" | "name" | "description" | "tags">;
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
        {agent.tags?.length ? (
          // the labels the platform keeps on the agent: a hint of what it is for, nothing more
          <span className="line-clamp-1 text-xs leading-4 text-muted-foreground/80">
            {agent.tags.join(" · ")}
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
 * The agent picker, in the top bar like a model picker: a button with the agent's name ("Adam ⌄")
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
  const triggerRef = useRef<HTMLButtonElement | null>(null);
  /** A thread: the choice that waits for the person's yes (a fork is not undone by going back). */
  const [confirm, setConfirm] = useState<Selection | null>(null);
  const [copying, setCopying] = useState(false);

  // a new chat falls back to the first agent; an existing thread names its own, listed or not
  const current = isNew
    ? selectedAgent(list, value)
    : value.agentId === null
      ? undefined
      : agentNamed(list, value.agentId);
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
  const releases = current?.releases;
  // an existing thread names its own agent even when the list does not (yet) hold it
  const shown: Pick<ApiAgent, "id" | "name" | "description" | "tags">[] =
    isNew || current || agentId === null ? list : [{ id: agentId, name }, ...list];
  const blocked = props.mode === "thread" ? props.blocked : undefined;

  /** A choice: a new chat just takes it; a thread asks first, because it makes a fork. */
  const choose = (next: Selection) => {
    if (props.mode === "new") return props.onChange(next);
    if (next.agentId === agentId && (next.release ?? null) === (release ?? null)) return;
    setConfirm(next);
  };
  const continuing = confirm
    ? (list.find((a) => a.id === confirm.agentId)?.name ?? confirm.agentId)
    : "";

  return (
    <>
      {/* not modal: a picker in the top bar does not need the page behind it hidden from a screen
          reader (axe reports a modal menu's aria-hidden siblings as focusable), and a click or a Tab
          outside closes it all the same */}
      <DropdownMenu modal={false} onOpenChange={(open) => open && retry()}>
        <DropdownMenuTrigger asChild>
          <Button
            ref={triggerRef}
            type="button"
            variant="ghost"
            data-slot="agent-menu-trigger"
            className={cn(
              // a button does not shrink by default; this one must give way to the state and the toggle
              "h-9 max-w-[min(100%,16rem)] min-w-0 shrink gap-1.5 rounded-full ps-3 pe-2.5 text-base font-medium text-foreground",
              className,
            )}
          >
            {/* the words of the name come first, so a screen reader hears "Agent: Adam · stable" */}
            <span className="sr-only">Agent: </span>
            <span className="min-w-0 truncate">
              <span className={current ? undefined : "capitalize"}>{name}</span>
              {release ? (
                // a phone's top bar has no room for it; it stays part of the name for a screen reader
                <span className="font-normal text-muted-foreground max-sm:sr-only">
                  {" "}
                  · {release}
                </span>
              ) : null}
            </span>
            <ChevronDownIcon aria-hidden="true" className="size-4 text-muted-foreground" />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent
          align="start"
          className="max-h-(--radix-dropdown-menu-content-available-height) w-[min(23.75rem,calc(100vw-1rem))] overflow-y-auto p-1.5"
        >
          {shown.length > 0 ? (
            <>
              <DropdownMenuLabel aria-hidden="true">Agents</DropdownMenuLabel>
              <DropdownMenuRadioGroup
                aria-label="Agents"
                value={agentId ?? ""}
                onValueChange={(id) => {
                  // the agent this chat has is no choice (a thread keeps it); a new chat takes it again
                  if (props.mode === "thread" && id === agentId) return;
                  choose({ agentId: id, release: null });
                }}
              >
                {shown.map((a) => (
                  <DropdownMenuRadioItem
                    key={a.id}
                    value={a.id}
                    textValue={a.name}
                    // another agent is a fork, which a turn that is going on cannot make
                    disabled={blocked !== undefined && a.id !== agentId}
                  >
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
                    onValueChange={(next) => choose({ agentId: current.id, release: next })}
                  >
                    {Object.entries(releases.channels).map(([channel, revision]) => (
                      <DropdownMenuRadioItem
                        key={`channel:${channel}`}
                        value={channel}
                        textValue={channel}
                        disabled={blocked !== undefined && channel !== release}
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
                        disabled={blocked !== undefined && revision !== release}
                        className="items-center py-1.5"
                      >
                        <span className="text-sm">{revision}</span>
                      </DropdownMenuRadioItem>
                    ))}
                  </DropdownMenuRadioGroup>
                </>
              ) : null}
              {props.mode === "thread" ? (
                <>
                  <DropdownMenuSeparator />
                  <p
                    data-slot="agent-menu-hint"
                    className="px-2.5 py-1.5 text-xs leading-4 text-muted-foreground"
                  >
                    {blocked ??
                      "A chat keeps its agent. Another agent, or another release, continues this conversation in a new chat."}
                  </p>
                </>
              ) : null}
            </>
          ) : (
            <DropdownMenuItem onSelect={retry}>Retry loading the agents</DropdownMenuItem>
          )}
          {error && list.length > 0 ? <Problem message={error} onRetry={retry} /> : null}
          {notice ?? <RegistryMenuNotice agents={agents} />}
        </DropdownMenuContent>
      </DropdownMenu>
      {props.mode === "thread" ? (
        <AlertDialog
          open={confirm !== null}
          onOpenChange={(open) => {
            if (!open && !copying) setConfirm(null);
          }}
        >
          <AlertDialogContent
            // the focus goes back to the picker, which is where the choice was made
            onCloseAutoFocus={(event) => {
              event.preventDefault();
              triggerRef.current?.focus();
            }}
          >
            <AlertDialogHeader>
              <AlertDialogTitle>Continue with {continuing} in a new chat?</AlertDialogTitle>
              <AlertDialogDescription>
                The conversation so far is copied; this chat stays as it is.
              </AlertDialogDescription>
            </AlertDialogHeader>
            <AlertDialogFooter>
              <AlertDialogCancel disabled={copying}>Cancel</AlertDialogCancel>
              <AlertDialogAction
                disabled={copying}
                onClick={(event) => {
                  event.preventDefault();
                  if (!confirm || props.mode !== "thread") return;
                  setCopying(true);
                  void props
                    .onContinue(confirm)
                    .then((made) => {
                      // made: the page goes to the new chat. Not made: the page says why.
                      if (!made) setConfirm(null);
                    })
                    .finally(() => setCopying(false));
                }}
              >
                {copying ? "Copying the conversation…" : "Continue in a new chat"}
              </AlertDialogAction>
            </AlertDialogFooter>
          </AlertDialogContent>
        </AlertDialog>
      ) : null}
    </>
  );
}
