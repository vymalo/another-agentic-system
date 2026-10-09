"use client";

import { PlugIcon, XIcon } from "lucide-react";
import { useState } from "react";
import { Hint } from "@/components/hint";
import { InlineStatus } from "@/components/inline-status";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuCheckboxItem,
  DropdownMenuContent,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import type { AgentCapabilities } from "@/features/agents/hooks/use-agent-capabilities";
import type { ApiToolServer } from "@/lib/api/types";
import type { ToolServersView } from "../hooks/use-tool-servers";
import { MAX_ATTACHED, nameOf, offeredFor, THREAD_TOOLS_URI, withServer } from "../lib/servers";
import { ServerIcon } from "./server-icon";

type Props = {
  /** What the deployment offers (`GET /api/tool-servers`). */
  view: ToolServersView;
  /** The agent the chat talks to: a server is offered only for the agents its `agents` names. */
  agentId: string | null;
  /** The ids attached to the thread, or chosen for the chat that is about to start. */
  chosen: readonly string[];
  /** Makes this the whole set: attaches and detaches. */
  onChange: (next: string[]) => void;
  /** A change is on its way: the choices wait for it. */
  busy?: boolean;
  /** The agent's card, read live: opening the menu reads it again. */
  capabilities: AgentCapabilities;
};

/**
 * The tools picker, in the composer's toolbar: a plug icon button, "Tools", that opens a menu of the servers
 * the deployment offers for this agent (icon, name, what it is for, a check on the attached ones),
 * and the attached ones as chips beside it, each with a button that takes it off. Where the choice
 * goes is the caller's: a new chat keeps it for the run that creates the thread (`vymalo.tools`),
 * an open thread sends it at once (`PUT /api/threads/{id}/tools`).
 *
 * Nothing is drawn when the deployment offers nothing for the agent and nothing is attached, and
 * when the list is not for this person (their roles hold no `thread.write`): a screen that has no
 * tools has no picker. A server attached earlier that the deployment no longer offers keeps its
 * chip, so it can be taken off. Radix's menu is the keyboard contract: arrows move, Space or
 * Enter toggle (the menu stays open for the next one), Escape closes and the focus goes back to
 * the button.
 */
export function ToolsPicker({
  view,
  agentId,
  chosen,
  onChange,
  busy = false,
  capabilities,
}: Props) {
  const offered = offeredFor(view.servers, agentId);
  // the tooltip of the trigger stays shut while its menu is open: the menu opens where it would be
  const [menuOpen, setMenuOpen] = useState(false);
  if (view.unavailable) return null;
  if (offered.length === 0 && chosen.length === 0 && !view.error) return null;

  const full = chosen.length >= MAX_ATTACHED;
  return (
    <>
      <DropdownMenu
        modal={false}
        onOpenChange={(open) => {
          setMenuOpen(open);
          // the list and the agent's card are read again each time: neither is ever cached
          if (open) {
            view.reload();
            capabilities.refresh();
          }
        }}
      >
        <Hint label="Tools" side="top" suppressed={menuOpen}>
          <DropdownMenuTrigger asChild>
            <Button
              type="button"
              variant="ghost"
              size="icon"
              aria-label="Tools"
              data-slot="tools-trigger"
              className="size-8 rounded-full text-muted-foreground"
            >
              <PlugIcon aria-hidden="true" />
            </Button>
          </DropdownMenuTrigger>
        </Hint>
        <DropdownMenuContent
          align="start"
          side="top"
          className="max-h-(--radix-dropdown-menu-content-available-height) w-[min(23.75rem,calc(100vw-1rem))] overflow-y-auto p-1.5"
        >
          <DropdownMenuLabel aria-hidden="true">Tools for this chat</DropdownMenuLabel>
          {offered.length === 0 ? (
            <p className="px-2.5 py-1.5 text-xs leading-4 text-muted-foreground">
              No tools are offered for this agent.
            </p>
          ) : (
            offered.map((server) => (
              <DropdownMenuCheckboxItem
                key={server.id}
                checked={chosen.includes(server.id)}
                // a choice does not close the menu: several are made in one visit
                onSelect={(event) => event.preventDefault()}
                onCheckedChange={(on) => onChange(withServer(chosen, server.id, on === true))}
                textValue={server.name}
                disabled={busy || (full && !chosen.includes(server.id))}
              >
                <ServerLabel server={server} />
              </DropdownMenuCheckboxItem>
            ))
          )}
          {view.error ? (
            <>
              <DropdownMenuSeparator />
              <p role="status" className="px-2.5 py-1.5 text-xs text-destructive">
                Could not refresh the tools: {view.error}
              </p>
            </>
          ) : null}
          <DropdownMenuSeparator />
          <p
            data-slot="tools-hint"
            className="px-2.5 py-1.5 text-xs leading-4 text-muted-foreground"
          >
            {capabilities.supports(THREAD_TOOLS_URI) === false
              ? "This agent does not use attached tools."
              : "The agent can use what is attached, from your next message on."}
          </p>
        </DropdownMenuContent>
      </DropdownMenu>
      {chosen.length > 0 ? (
        <ul aria-label="Attached tools" className="flex min-w-0 flex-wrap items-center gap-1.5">
          {chosen.map((id) => {
            const name = nameOf(view.servers, id);
            return (
              <li
                key={id}
                data-slot="tool-chip"
                data-server={id}
                className="inline-flex h-7 max-w-full min-w-0 items-center gap-1.5 rounded-full bg-muted ps-2 pe-0.5 text-[0.8125rem] text-foreground"
              >
                <ServerIcon server={view.servers.find((s) => s.id === id)} />
                <span className="min-w-0 truncate">{name}</span>
                <Hint label={`Remove ${name}`} side="top">
                  <button
                    type="button"
                    aria-label={`Remove ${name}`}
                    disabled={busy}
                    onClick={() => onChange(withServer(chosen, id, false))}
                    className="inline-flex size-6 shrink-0 cursor-pointer items-center justify-center rounded-full text-muted-foreground outline-none hover:bg-background hover:text-foreground focus-visible:ring-3 focus-visible:ring-ring/50 disabled:pointer-events-none disabled:opacity-50"
                  >
                    <XIcon aria-hidden="true" className="size-3.5" />
                  </button>
                </Hint>
              </li>
            );
          })}
        </ul>
      ) : null}
    </>
  );
}

/** A server in the menu: its icon, its name and, in one line, what it is for. */
function ServerLabel({ server }: { server: ApiToolServer }) {
  return (
    <>
      <ServerIcon server={server} className="mt-0.5 size-5" />
      <span className="flex min-w-0 flex-col gap-0.5">
        <span className="text-sm leading-5 font-medium">{server.name}</span>
        {server.description ? (
          <span className="line-clamp-2 text-xs leading-4 text-muted-foreground">
            {server.description}
          </span>
        ) : null}
      </span>
    </>
  );
}

/**
 * Said before the person sends: the agent's card does not list `thread-tools/v1`, so the tools
 * attached to the chat are not sent to it (the thread keeps them; a person can still take them
 * off). An agent whose card could not be read is "could not check", never "can": ADR 0008 fails
 * closed. Nothing while no tools are attached or the card has not been read yet.
 */
export function ToolsWarning({
  chosen,
  capabilities,
  agentName,
}: {
  chosen: readonly string[];
  capabilities: AgentCapabilities;
  agentName: string;
}) {
  if (chosen.length === 0) return null;
  const listed = capabilities.supports(THREAD_TOOLS_URI);
  if (capabilities.status === "ready" && listed === false) {
    return (
      <InlineStatus tone="warning" role="status">
        <span data-slot="tools-warning">
          {agentName} cannot use attached tools, so they will not be sent to it.
        </span>
      </InlineStatus>
    );
  }
  if (capabilities.status === "unreadable") {
    return (
      <InlineStatus tone="warning" role="status">
        <span data-slot="tools-warning">
          Could not check whether {agentName} can use attached tools.
        </span>
      </InlineStatus>
    );
  }
  return null;
}
