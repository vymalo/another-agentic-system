import type { ApiActor } from "@/lib/api/types";

/** `coder · coder-r47`: who posted it, and which revision produced it. */
export function ActorLabel({ actor }: { actor: ApiActor | undefined }) {
  if (!actor) return null;
  return (
    <span data-slot="actor-label" className="whitespace-nowrap text-xs text-muted-foreground">
      {actor.name}
      {actor.revision ? ` · ${actor.revision}` : ""}
    </span>
  );
}
