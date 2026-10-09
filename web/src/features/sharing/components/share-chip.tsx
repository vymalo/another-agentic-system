import { GlobeIcon, LinkIcon, UsersIcon } from "lucide-react";
import type { ApiShare } from "@/lib/api/types";
import { cn } from "@/lib/utils";
import { chipLabel } from "../lib/sharing";

/** The icon of a share: the world for a public one, people for a signed-in one, a link when paused. */
function ShareIcon({ share, className }: { share: ApiShare; className?: string }) {
  const Icon =
    share.effective === "public"
      ? GlobeIcon
      : share.effective === "internal"
        ? UsersIcon
        : LinkIcon;
  return <Icon aria-hidden="true" className={className} />;
}

/**
 * The top bar's chip for a thread that is shared: the icon of who can read it (people for the signed-in,
 * the world for anybody, a link while it is paused) and, from `sm` up, the words, "Shared · signed-in" or
 * "Shared · public": who can read a conversation is not left to a tooltip a keyboard cannot reach. A
 * phone's bar has no room for the words (the title is what gives way elsewhere): there the icon stays,
 * with the words for a screen reader. Anybody is on the warning colours, and the icon is a different
 * shape besides, never a colour alone.
 */
export function ShareChip({ share }: { share: ApiShare }) {
  const open = share.effective === "public";
  return (
    <span
      data-slot="share-chip"
      data-effective={share.effective}
      className={cn(
        "inline-flex h-7 shrink-0 items-center gap-1.5 rounded-full px-2.5 text-[0.8125rem] max-sm:size-7 max-sm:justify-center max-sm:px-0",
        open ? "bg-warning-soft text-warning" : "bg-muted text-foreground",
      )}
    >
      <ShareIcon
        share={share}
        className={cn("size-3.5", open ? "text-warning" : "text-muted-foreground")}
      />
      <span className="max-sm:sr-only">{chipLabel(share)}</span>
    </span>
  );
}

/** The mark after a shared thread's title in the sidebar: the icon, and the words for a screen reader. */
export function ShareMark({ share }: { share: ApiShare }) {
  return (
    <span data-slot="share-mark" className="flex shrink-0 items-center">
      <ShareIcon share={share} className="size-3.5 text-muted-foreground" />
      <span className="sr-only">, {chipLabel(share).toLowerCase()}</span>
    </span>
  );
}
