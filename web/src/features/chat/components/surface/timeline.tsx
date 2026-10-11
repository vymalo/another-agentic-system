"use client";

import type { ReactNode } from "react";
import { readEvents, STATE_WORDS } from "@/features/chat/lib/a2ui/status";
import { plainName } from "@/features/chat/lib/files";
import { cn } from "@/lib/utils";
import { StateIcon } from "./status-icon";

/*
 * The `Timeline` component of the UI catalog (docs/api/ui-catalog-v1.md, version 6): up to fifty events in order,
 * each a time, a title, an optional description and a state (done when the agent says none). The state is said in
 * words beside its icon. Output only; every string is the agent's, drawn as text.
 */

type Rec = Record<string, unknown>;
const str = (v: unknown): string | undefined => (typeof v === "string" ? v : undefined);

/** The Timeline of a surface; the props are those of the lowered component (`prepare.ts`). */
export function TimelineView(props: Rec): ReactNode {
  const events = readEvents(props.events);
  const title = plainName(str(props.title) ?? "", 120);
  return (
    <div data-slot="timeline" className="flex min-w-0 flex-col gap-2">
      {title ? <p className="text-sm font-medium [overflow-wrap:anywhere]">{title}</p> : null}
      <ol aria-label={title || "Timeline"} className="m-0 flex min-w-0 list-none flex-col p-0">
        {events.map((event, i) => {
          const last = i === events.length - 1;
          return (
            // the events are written once and never reordered: the position is the identity of a row
            // biome-ignore lint/suspicious/noArrayIndexKey: see above
            <li key={i} data-state={event.state} className="flex min-w-0 gap-3">
              <span aria-hidden="true" className="flex flex-col items-center">
                <StateIcon state={event.state} />
                {last ? null : <span className="w-px flex-1 bg-border" />}
              </span>
              <div className={cn("flex min-w-0 flex-1 flex-col gap-0.5", !last && "pb-4")}>
                <p className="text-xs text-muted-foreground">
                  <span className="tabular-nums">{event.time}</span>
                  <span aria-hidden="true"> · </span>
                  <span>{STATE_WORDS[event.state]}</span>
                </p>
                <p className="text-sm font-medium [overflow-wrap:anywhere]">{event.title}</p>
                {event.description ? (
                  <p className="text-sm whitespace-pre-wrap [overflow-wrap:anywhere]">
                    {event.description}
                  </p>
                ) : null}
              </div>
            </li>
          );
        })}
      </ol>
    </div>
  );
}
