"use client";

import { ArrowDownIcon, ArrowUpIcon, Trash2Icon } from "lucide-react";
import { Button } from "@/components/ui/button";
import type { KeptQuestion } from "../lib/editor";

type Props = {
  q: KeptQuestion;
  index: number;
  count: number;
  baseId: string;
  busy: boolean;
  /** What the server refused about this question; the card cannot be edited, so only its words are shown. */
  refusal: { message: string } | null;
  onMove: (delta: -1 | 1) => void;
  onRemove: () => void;
};

/**
 * A question of a kind this page cannot edit (a long list, a scale, a ranking, or a kind a later catalog adds). It is written
 * back as the very object that was read, so saving the prompt never changes it. It can still be moved and removed.
 */
export function KeptQuestionCard({
  q,
  index,
  count,
  baseId,
  busy,
  refusal,
  onMove,
  onRemove,
}: Props) {
  const n = index + 1;
  const errorId = `${baseId}-error`;
  const name = q.question || q.id || `question ${n}`;
  return (
    <li
      data-slot="kept-question"
      data-question={q.key}
      className="flex flex-col gap-2 rounded-xl border border-dashed bg-muted/40 px-4 py-4"
    >
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h3
          id={baseId}
          tabIndex={-1}
          aria-describedby={refusal ? errorId : undefined}
          className="text-sm font-medium outline-none"
        >
          Question {n}
        </h3>
        <div className="flex gap-1">
          <Button
            type="button"
            variant="ghost"
            size="icon-sm"
            id={`${baseId}-up`}
            aria-label={`Move question ${n} up`}
            disabled={busy || index === 0}
            onClick={() => onMove(-1)}
          >
            <ArrowUpIcon aria-hidden="true" />
          </Button>
          <Button
            type="button"
            variant="ghost"
            size="icon-sm"
            id={`${baseId}-down`}
            aria-label={`Move question ${n} down`}
            disabled={busy || index === count - 1}
            onClick={() => onMove(1)}
          >
            <ArrowDownIcon aria-hidden="true" />
          </Button>
          <Button
            type="button"
            variant="ghost"
            size="icon-sm"
            aria-label={`Remove question ${n}`}
            disabled={busy}
            onClick={onRemove}
          >
            <Trash2Icon aria-hidden="true" />
          </Button>
        </div>
      </div>
      <p className="text-sm text-muted-foreground">
        <span className="font-medium break-words text-foreground">“{name}”</span> is {q.phrase}.
        This page cannot edit it yet; it is kept as it is.
      </p>
      {refusal ? (
        <p id={errorId} role="alert" className="text-sm text-destructive">
          {refusal.message}
        </p>
      ) : null}
    </li>
  );
}
