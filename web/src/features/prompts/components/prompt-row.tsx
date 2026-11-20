"use client";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import type { ApiPromptSummary } from "@/lib/api/types";
import { asksWords, sharedLine } from "../lib/library";
import type { SharingControl } from "../lib/share";

type Props = {
  prompt: ApiPromptSummary;
  /** What the person may do about who reads it (`sharingControl`): nothing while `prompts.share` is unknown. */
  sharing: SharingControl | null;
  /** The row's Stop sharing is in flight: the button says so and takes no second press. */
  stopping: boolean;
  onEdit: (prompt: ApiPromptSummary) => void;
  onDelete: (prompt: ApiPromptSummary) => void;
  onView: (prompt: ApiPromptSummary) => void;
  onCopy: (prompt: ApiPromptSummary) => void;
  onShare: (prompt: ApiPromptSummary) => void;
  onStopSharing: (prompt: ApiPromptSummary) => void;
};

/**
 * One prompt of the list. Its words are drawn as plain text, never as Markdown or HTML: for a shared prompt they are somebody
 * else's. A shared prompt shows no author (the API gives none) and offers only to be read and to be copied; an own one shows who
 * it is shared with, as the server returned it, and the one sharing control the person may use. Every button names its prompt.
 */
export function PromptRow({
  prompt,
  sharing,
  stopping,
  onEdit,
  onDelete,
  onView,
  onCopy,
  onShare,
  onStopSharing,
}: Props) {
  const shared = sharedLine(prompt.own ? prompt.audience : undefined);
  const title = `“${prompt.title}”`;
  return (
    <li
      data-slot="prompt-row"
      data-prompt={prompt.id}
      data-own={prompt.own ? "true" : "false"}
      className="flex flex-col gap-3 rounded-xl border bg-card px-4 py-4 text-card-foreground"
    >
      <div className="flex min-w-0 flex-col gap-1">
        <div className="flex flex-wrap items-baseline gap-x-3 gap-y-1">
          <h3 className="min-w-0 text-base leading-6 font-medium break-words">{prompt.title}</h3>
          {prompt.own ? null : <Badge variant="secondary">Shared with you</Badge>}
        </div>
        <p className="font-mono text-sm break-all text-muted-foreground">/{prompt.name}</p>
        {prompt.description ? (
          <p className="line-clamp-2 text-sm break-words whitespace-pre-line text-muted-foreground">
            {prompt.description}
          </p>
        ) : null}
        <p className="text-xs text-muted-foreground">
          {asksWords(prompt.variableCount)}
          {shared ? <span className="break-words"> · {shared}</span> : null}
        </p>
      </div>
      <div className="flex flex-wrap gap-2">
        {prompt.own ? (
          <>
            <Button
              type="button"
              size="sm"
              variant="outline"
              data-opener={`edit:${prompt.id}`}
              aria-label={`Edit ${title}`}
              onClick={() => onEdit(prompt)}
            >
              Edit
            </Button>
            {sharing === "share" ? (
              <Button
                type="button"
                size="sm"
                variant="outline"
                data-opener={`share:${prompt.id}`}
                aria-label={`Share ${title}`}
                onClick={() => onShare(prompt)}
              >
                Share
              </Button>
            ) : null}
            {sharing === "change" ? (
              <Button
                type="button"
                size="sm"
                variant="outline"
                data-opener={`share:${prompt.id}`}
                aria-label={`Change sharing ${title}`}
                onClick={() => onShare(prompt)}
              >
                Change sharing
              </Button>
            ) : null}
            {sharing === "stop" ? (
              <Button
                type="button"
                size="sm"
                variant="outline"
                data-opener={`stop:${prompt.id}`}
                aria-label={stopping ? `Stopping sharing ${title}` : `Stop sharing ${title}`}
                aria-busy={stopping || undefined}
                disabled={stopping}
                onClick={() => onStopSharing(prompt)}
              >
                {stopping ? "Stopping…" : "Stop sharing"}
              </Button>
            ) : null}
            <Button
              type="button"
              size="sm"
              variant="ghost"
              data-opener={`delete:${prompt.id}`}
              aria-label={`Delete ${title}`}
              onClick={() => onDelete(prompt)}
            >
              Delete
            </Button>
          </>
        ) : (
          <>
            <Button
              type="button"
              size="sm"
              variant="outline"
              data-opener={`view:${prompt.id}`}
              aria-label={`View ${title}`}
              onClick={() => onView(prompt)}
            >
              View
            </Button>
            <Button
              type="button"
              size="sm"
              variant="outline"
              data-opener={`copy:${prompt.id}`}
              aria-label={`Copy to my prompts: ${title}`}
              onClick={() => onCopy(prompt)}
            >
              Copy to my prompts
            </Button>
          </>
        )}
      </div>
    </li>
  );
}
