import { PlugIcon } from "lucide-react";
import type { ApiToolServer } from "@/lib/api/types";
import { cn } from "@/lib/utils";
import { iconSrc } from "../lib/icon";

/**
 * An MCP server's icon, drawn as the deployment configured it: a `data:` image through an `<img>`
 * (never inlined, so an SVG runs nothing and loads nothing), else the generic plug. Nothing else is
 * ever a source: an icon that is a URL is no icon, and nothing is fetched for it (open question 38,
 * `lib/icon.ts`). Decorative: whoever draws it names the server in text beside it.
 */
export function ServerIcon({
  server,
  className,
}: {
  server: Pick<ApiToolServer, "icon"> | undefined;
  className?: string;
}) {
  const src = iconSrc(server?.icon);
  return src ? (
    // biome-ignore lint/performance/noImgElement: a data: URI from the configuration, nothing to optimise
    <img
      data-slot="server-icon"
      src={src}
      alt=""
      width={16}
      height={16}
      draggable={false}
      referrerPolicy="no-referrer"
      className={cn("size-4 shrink-0 rounded-[3px] object-contain", className)}
    />
  ) : (
    <PlugIcon
      data-slot="server-icon"
      aria-hidden="true"
      className={cn("size-4 shrink-0 text-muted-foreground", className)}
    />
  );
}
