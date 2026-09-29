import type { ActionContent } from "@/features/chat/lib/agui/vymalo";
import { ActorLabel } from "../actor-label";

/**
 * What the owner did on an A2UI surface: a quiet line, the agent's own words for the action
 * (`name`) as text. The action has no message in the transcript (docs/api/agui.md, "Actions").
 */
export function ActionLine({ data }: { data: ActionContent }) {
  return (
    <p
      data-slot="action-line"
      className="flex flex-wrap items-baseline gap-x-2 text-sm text-muted-foreground"
    >
      <span className="[overflow-wrap:anywhere]">
        Chose <strong className="font-medium text-foreground">{data.name}</strong>
        {data.sourceComponentId && data.sourceComponentId !== data.name
          ? ` (${data.sourceComponentId})`
          : ""}
      </span>
      <ActorLabel actor={data.actor} />
    </p>
  );
}
