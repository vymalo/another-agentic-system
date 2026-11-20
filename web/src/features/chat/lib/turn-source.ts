import { surfacePlaceholders } from "@/features/chat/lib/a2ui/placeholder";
import { ACTIVITY, activityPartName } from "@/features/chat/lib/agui/vymalo";
import type { TextRole } from "@/features/chat/lib/working";

type Part = { type: string; name?: string; text?: string; data?: unknown };

/**
 * What Copy and Quote take of an agent's turn: the source text of every part that is the answer (not working text), in order,
 * and, where a surface was drawn, one `[Interface: …]` line for it. Parts are joined by a blank line.
 */
export function turnSource(content: readonly Part[], roles: ReadonlyMap<number, TextRole>): string {
  const surface = activityPartName(ACTIVITY.surface);
  return content
    .flatMap((p, i) => {
      if (p.type === "text") return p.text?.trim() && roles.get(i) !== "working" ? [p.text] : [];
      if (p.type === "data" && p.name === surface) {
        return surfacePlaceholders(
          (p.data as { a2ui_operations?: unknown } | undefined)?.a2ui_operations,
        );
      }
      return [];
    })
    .join("\n\n");
}
