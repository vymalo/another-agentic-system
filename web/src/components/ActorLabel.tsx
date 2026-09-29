import type { ApiActor } from "@/api/types";

/** `coder · coder-r47`: who posted it, and which revision produced it. */
export function ActorLabel({ actor }: { actor: ApiActor | undefined }) {
  if (!actor) return null;
  return (
    <span className="actor">
      {actor.name}
      {actor.revision ? ` · ${actor.revision}` : ""}
    </span>
  );
}
