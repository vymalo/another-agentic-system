"use client";

import { ChevronLeftIcon, ChevronRightIcon } from "lucide-react";
import { useRouter } from "next/navigation";
import type { FC } from "react";
import { TooltipIconButton } from "@/components/assistant-ui/elements/tooltip-icon-button";
import { useThreadBranches } from "@/features/threads/components/branches-provider";
import type { ApiBranchPoint } from "@/lib/api/types";

type PickerProps = {
  /** Which version is shown, from 0. */
  index: number;
  /** How many versions there are (at least 2). */
  total: number;
  onSelect: (index: number) => void;
};

/**
 * `‹ 2/3 ›` under a message that has other versions (ADR 0029): an edit of a message is a new
 * branch of the conversation and the old one stays, one click away. Each version is a thread of
 * its own, so choosing one goes to it (`onSelect`). The first version has no way back and the last
 * no way on: the arrow is disabled there, never wrapping round. What the eye reads as `2/3` is
 * "Version 2 of 3" in a polite live region for a screen reader (the page of the chosen version
 * puts the focus on the message, which is how the change is heard).
 */
export const BranchPicker: FC<PickerProps> = ({ index, total, onSelect }) => (
  <fieldset
    data-slot="branch-picker"
    className="m-0 flex min-w-0 items-center border-0 p-0 text-xs text-muted-foreground"
  >
    <legend className="sr-only">Versions of this message</legend>
    <TooltipIconButton
      tooltip="Previous version"
      aria-label="Previous version"
      side="bottom"
      className="size-7 rounded-md p-0 [&_svg]:size-3.5"
      disabled={index <= 0}
      onClick={() => onSelect(index - 1)}
    >
      <ChevronLeftIcon aria-hidden="true" />
    </TooltipIconButton>
    <span aria-hidden="true" className="min-w-8 text-center font-medium tabular-nums">
      {index + 1}/{total}
    </span>
    <span role="status" className="sr-only">
      Version {index + 1} of {total}
    </span>
    <TooltipIconButton
      tooltip="Next version"
      aria-label="Next version"
      side="bottom"
      className="size-7 rounded-md p-0 [&_svg]:size-3.5"
      disabled={index >= total - 1}
      onClick={() => onSelect(index + 1)}
    >
      <ChevronRightIcon aria-hidden="true" />
    </TooltipIconButton>
  </fieldset>
);

/** The picker of a message that has versions: an arrow goes to the thread of the chosen one. */
const VersionsOf: FC<{ point: ApiBranchPoint }> = ({ point }) => {
  const router = useRouter();
  return (
    <BranchPicker
      index={point.index}
      total={point.siblings.length}
      onSelect={(i) => {
        const to = point.siblings[i];
        if (to) router.push(`/threads/${to.threadId}#m-${to.seq}`);
      }}
    />
  );
};

/**
 * The picker of the message of the open thread that is at `seq`, when it has other versions: it
 * goes to the chosen thread, scrolled to the same message there (`#m-<seq>` of that thread). The
 * router is only asked for by a message that has versions, so a bubble drawn outside the app's
 * pages (a test of a surface, say) needs none.
 */
export const MessageBranches: FC<{ seq: number | undefined }> = ({ seq }) => {
  const point = useThreadBranches().pointAt(seq);
  return point ? <VersionsOf point={point} /> : null;
};
