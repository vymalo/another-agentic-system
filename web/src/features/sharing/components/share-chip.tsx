import { GlobeIcon, LinkIcon, UsersIcon } from "lucide-react";
import type { ApiShare } from "@/lib/api/types";
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
 * The top bar's chip for a thread that is shared: "Shared · signed-in" or "Shared · public", words
 * and an icon, never only a colour. A phone's bar has no room for the words (the title is what gives
 * way elsewhere): the icon stays and the words are for a screen reader.
 */
export function ShareChip({ share }: { share: ApiShare }) {
  return (
    <span
      data-slot="share-chip"
      data-effective={share.effective}
      className="inline-flex h-7 shrink-0 items-center gap-1.5 rounded-full bg-muted px-2.5 text-[0.8125rem] text-foreground max-sm:px-2"
    >
      <ShareIcon share={share} className="size-3.5 text-muted-foreground" />
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
