import type { ToolsContent } from "@/features/chat/lib/agui/vymalo";
import type { ApiToolServer } from "@/lib/api/types";
import { nameOf } from "./servers";

/** "Web search", "Web search and GitHub", "Web search, GitHub and Files". */
const list = (names: string[]): string =>
  names.length <= 1
    ? (names[0] ?? "")
    : `${names.slice(0, -1).join(", ")} and ${names[names.length - 1]}`;

/**
 * The words of the line a `vymalo.tools` card is: "Web search attached" or "GitHub detached", one
 * line for each of the two members it may have. The names are the deployment's (from the list the
 * screen read); a server the list does not have is named by its id.
 */
export function toolsLines(content: ToolsContent, servers: readonly ApiToolServer[]): string[] {
  const named = (ids: string[] | undefined) => (ids ?? []).map((id) => nameOf(servers, id));
  return [
    ...(content.attached?.length ? [`${list(named(content.attached))} attached`] : []),
    ...(content.detached?.length ? [`${list(named(content.detached))} detached`] : []),
  ];
}
