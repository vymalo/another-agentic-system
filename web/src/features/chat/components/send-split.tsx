"use client";

import { ArrowUpIcon, ChevronDownIcon, SquareIcon } from "lucide-react";
import { useId, useState } from "react";
import { Hint } from "@/components/hint";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import type { SendMode } from "@/features/chat/lib/agui/vymalo";
import { sendHint, stopHint } from "@/features/chat/lib/send";

type Props = {
  /** Who works: the name the hints use ("Adam"). */
  agent: string;
  /** Whether the agent's card lists `steer/v1` (`readsWhen`): null when it could not be read. */
  steers: boolean | null;
  /** The conversation is not on screen yet: a send now would replace its turns. */
  loading: boolean;
  onSend: (mode: SendMode) => void;
  /** Where the focus goes when the menu closes: the box, because the button that opened it is gone. */
  onClosed: () => void;
};

const KEYS = {
  steer: "Enter",
  interrupt: "Ctrl/⌘ Shift Enter",
} as const;

/**
 * Send while the agent works (ADR 0036): a round **Send** (Enter) beside a chevron that opens a
 * menu of the two ways, "Send" (the agent reads it at its next step, or after this turn) and
 * "Stop and send" (the agent stops and starts again with the message). It is a group of two
 * buttons, the menu's trigger the second: a person who only presses Send never meets the menu.
 * Radix's menu is the keyboard contract (arrows, Enter, Escape).
 */
export function SendSplit({ agent, steers, loading, onSend, onClosed }: Props) {
  const hintId = useId();
  const [menuOpen, setMenuOpen] = useState(false);
  const hint = sendHint(agent, steers);
  const disabled = loading;
  return (
    <div data-slot="send-split" className="flex shrink-0 items-stretch">
      <Hint label={`Send: ${hint}`} side="top">
        <Button
          type="button"
          aria-label="Send"
          aria-describedby={hintId}
          aria-keyshortcuts="Enter"
          // a disabled button has no tooltip: the reason is the native title
          {...(loading ? { title: "Loading the conversation…" } : {})}
          disabled={disabled}
          data-slot="send"
          className="h-9 w-9 rounded-s-full rounded-e-none disabled:bg-muted disabled:text-muted-foreground disabled:opacity-100 [&_svg:not([class*='size-'])]:size-4.5"
          onClick={() => onSend("steer")}
        >
          <ArrowUpIcon aria-hidden="true" />
        </Button>
      </Hint>
      <span id={hintId} className="sr-only">
        {hint}
      </span>
      <DropdownMenu modal={false} onOpenChange={setMenuOpen}>
        <Hint label="Delivery options" side="top" suppressed={menuOpen}>
          <DropdownMenuTrigger asChild>
            <Button
              type="button"
              aria-label="Delivery options"
              disabled={disabled}
              data-slot="send-menu"
              className="h-9 w-7 rounded-s-none rounded-e-full border-s-primary-foreground/25 px-0 disabled:bg-muted disabled:text-muted-foreground disabled:opacity-100 [&_svg:not([class*='size-'])]:size-3.5"
            >
              <ChevronDownIcon aria-hidden="true" />
            </Button>
          </DropdownMenuTrigger>
        </Hint>
        <DropdownMenuContent
          align="end"
          side="top"
          className="w-[min(21rem,calc(100vw-1rem))] p-1.5"
          onCloseAutoFocus={(event) => {
            event.preventDefault();
            onClosed();
          }}
        >
          <DropdownMenuItem
            data-slot="send-steer"
            aria-keyshortcuts="Enter"
            onSelect={() => onSend("steer")}
          >
            <ArrowUpIcon aria-hidden="true" />
            <span className="flex min-w-0 flex-1 flex-col gap-0.5">
              <span className="text-foreground">Send</span>
              <span className="text-xs leading-4 text-muted-foreground">{hint}</span>
            </span>
            <kbd className="shrink-0 text-xs text-muted-foreground">{KEYS.steer}</kbd>
          </DropdownMenuItem>
          <DropdownMenuItem
            data-slot="send-interrupt"
            aria-keyshortcuts="Control+Shift+Enter Meta+Shift+Enter"
            onSelect={() => onSend("interrupt")}
          >
            <SquareIcon aria-hidden="true" className="fill-current" />
            <span className="flex min-w-0 flex-1 flex-col gap-0.5">
              <span className="text-foreground">Stop and send</span>
              <span className="text-xs leading-4 text-muted-foreground">{stopHint(agent)}</span>
            </span>
            <kbd className="shrink-0 text-xs text-muted-foreground">{KEYS.interrupt}</kbd>
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
    </div>
  );
}
