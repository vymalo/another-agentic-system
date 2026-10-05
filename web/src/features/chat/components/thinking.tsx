"use client";

import { ChevronRightIcon } from "lucide-react";
import { type FC, useId, useState, useSyncExternalStore } from "react";
import { cn } from "@/lib/utils";

/*
 * What the agent's model thought before it answered (ADR 0044, docs/api/agui.md "Reasoning"): a
 * block that is **closed** until the person opens it, above the words of the turn. The label is
 * "Thinking", with the shimmer of the starting line while the model is still writing it; the text is
 * what the model wrote, drawn as text (it is untrusted, so no markup is read from it) and kept in the
 * block's own scroll, so a long one never pushes the answer away. It grows while it is open.
 */

/**
 * Which reasoning blocks the person opened, by the reasoning's id. The draft that is drawn while the model
 * writes and the reasoning part the transcript has when the log says it are two components for one block, so
 * the state lives here: a block that was open stays open when the log's text takes the draft's place.
 */
const opened = new Set<string>();
const listeners = new Set<() => void>();
const subscribe = (listener: () => void) => {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
};
function setOpened(id: string, open: boolean) {
  if (open) opened.add(id);
  else opened.delete(id);
  for (const listener of [...listeners]) listener();
}

/** The reasoning id a reasoning part of the transcript carries (`providerMetadata.agui.reasoningId`), if it does. */
export function reasoningIdOf(part: { providerMetadata?: unknown }): string | undefined {
  const meta = part.providerMetadata;
  if (typeof meta !== "object" || meta === null) return undefined;
  const agui = (meta as Record<string, unknown>).agui;
  if (typeof agui !== "object" || agui === null) return undefined;
  const id = (agui as Record<string, unknown>).reasoningId;
  return typeof id === "string" && id !== "" ? id : undefined;
}

export const Thinking: FC<{
  /** What the model wrote. */
  text: string;
  /** The model is still writing it: the draft, not the log's. */
  streaming?: boolean;
  /** The id of the reasoning, for the test and the panel's link to it. */
  id?: string;
}> = ({ text, streaming = false, id }) => {
  const [local, setLocal] = useState(false);
  const remembered = useSyncExternalStore(
    subscribe,
    () => (id !== undefined ? opened.has(id) : false),
    () => false,
  );
  const open = id !== undefined ? remembered : local;
  const toggle = () => (id !== undefined ? setOpened(id, !open) : setLocal(!open));
  const panel = useId();
  return (
    <div
      data-slot="thinking"
      data-state={open ? "open" : "closed"}
      data-streaming={streaming ? "true" : "false"}
      {...(id ? { "data-message-id": id } : {})}
      className="min-w-0"
    >
      <button
        type="button"
        aria-expanded={open}
        aria-controls={panel}
        onClick={toggle}
        className="group/thinking inline-flex cursor-pointer items-center gap-1 rounded-md py-0.5 text-sm text-muted-foreground outline-none hover:text-foreground focus-visible:ring-3 focus-visible:ring-ring/50"
      >
        <ChevronRightIcon
          aria-hidden="true"
          className={cn(
            "size-3.5 transition-transform motion-reduce:transition-none",
            open && "rotate-90",
          )}
        />
        <span className={cn(streaming && "text-shimmer")}>Thinking</span>
      </button>
      {open ? (
        <section
          id={panel}
          data-slot="thinking-text"
          aria-label="Thinking"
          // a draft is announced when it is whole, as a reply is
          aria-live="off"
          aria-busy={streaming}
          className="mt-1 max-h-72 overflow-y-auto border-l-2 border-border pl-3 text-sm leading-6 whitespace-pre-wrap text-muted-foreground [overflow-wrap:anywhere]"
        >
          {text}
        </section>
      ) : null}
    </div>
  );
};
