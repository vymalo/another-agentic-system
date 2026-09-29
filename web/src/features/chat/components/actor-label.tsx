import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import type { ApiActor } from "@/lib/api/types";

/**
 * `coder · coder-r47`: who posted it, and which revision produced it. With `at`, hovering shows
 * when it happened (the time is secondary, so the label is not made a tab stop for it).
 */
export function ActorLabel({ actor, at }: { actor: ApiActor | undefined; at?: Date | undefined }) {
  if (!actor) return null;
  const label = (
    <span data-slot="actor-label" className="whitespace-nowrap text-xs text-muted-foreground">
      {actor.name}
      {actor.revision ? ` · ${actor.revision}` : ""}
    </span>
  );
  if (!at || Number.isNaN(at.getTime())) return label;
  return (
    <Tooltip>
      <TooltipTrigger asChild>{label}</TooltipTrigger>
      <TooltipContent side="bottom">
        <time dateTime={at.toISOString()}>{at.toLocaleString()}</time>
      </TooltipContent>
    </Tooltip>
  );
}
