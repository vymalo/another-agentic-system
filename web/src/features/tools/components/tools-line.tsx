"use client";

import { PlugIcon } from "lucide-react";
import type { ToolsContent } from "@/features/chat/lib/agui/vymalo";
import { toolsLines } from "../lib/line";
import { useToolServerList } from "./tool-servers-context";

/**
 * What a `vymalo.tools` card says, as the muted line it is in the conversation: "Web search
 * attached" or "GitHub detached", with a plug for the glance, in the voice of the person's own
 * acts (a rename, a fork): a thing that happened in the conversation, not a step of the agent's
 * work. The names are the deployment's; a server it no longer lists is its id. It is not a live
 * region: a replay would announce every old attachment.
 */
export function ToolsLine({ content }: { content: ToolsContent }) {
  const servers = useToolServerList();
  const lines = toolsLines(content, servers);
  if (lines.length === 0) return null;
  return (
    <div data-slot="tools-lines" className="flex min-w-0 flex-col gap-0.5">
      {lines.map((line) => (
        <p
          key={line}
          data-slot="tools-line"
          className="flex min-w-0 items-center gap-2 text-[0.8125rem] text-muted-foreground"
        >
          <PlugIcon aria-hidden="true" className="size-3.5 shrink-0" />
          <span className="min-w-0 [overflow-wrap:anywhere]">{line}</span>
        </p>
      ))}
    </div>
  );
}
