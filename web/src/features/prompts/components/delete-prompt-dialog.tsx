"use client";

import { useState } from "react";
import { InlineStatus } from "@/components/inline-status";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { buttonVariants } from "@/components/ui/button";
import type { ApiPromptSummary } from "@/lib/api/types";
import type { DeleteResult } from "../hooks/use-prompt-library";

type Props = {
  /** The row as it was when the person pressed Delete: its `revision` is the one sent. */
  prompt: ApiPromptSummary;
  onDelete: (id: string, revision: string) => Promise<DeleteResult>;
  onClose: () => void;
};

/**
 * Deleting is asked first, says what goes, and stays open until the outcome is known. It sends the `revision` of the row, so a
 * prompt changed elsewhere since the page read it is not deleted blind (409): the dialog says so, the list is read again, and
 * the person decides again. A prompt that is already gone counts as deleted.
 */
export function DeletePromptDialog({ prompt, onDelete, onClose }: Props) {
  const [pending, setPending] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const shared = (prompt.audience?.length ?? 0) > 0;
  return (
    <AlertDialog
      open
      onOpenChange={(open) => {
        if (!open && !pending) onClose();
      }}
    >
      <AlertDialogContent data-slot="delete-prompt-dialog">
        <AlertDialogHeader>
          <AlertDialogTitle className="break-words">Delete “{prompt.title}”?</AlertDialogTitle>
          <AlertDialogDescription asChild>
            <div className="flex flex-col gap-2">
              <p>
                This removes it from your prompts
                {shared ? ", and from the people it is shared with" : ""}. Messages you already sent
                with it do not change. This cannot be undone.
              </p>
            </div>
          </AlertDialogDescription>
        </AlertDialogHeader>
        {problem ? (
          <InlineStatus tone="error" role="alert">
            {problem}
          </InlineStatus>
        ) : null}
        <AlertDialogFooter>
          <AlertDialogCancel disabled={pending}>Cancel</AlertDialogCancel>
          <AlertDialogAction
            disabled={pending}
            className={buttonVariants({ variant: "destructive" })}
            onClick={async (event) => {
              // the dialog stays open until it is known whether the prompt is gone
              event.preventDefault();
              setPending(true);
              setProblem(null);
              const result = await onDelete(prompt.id, prompt.revision);
              setPending(false);
              if (result.ok) onClose();
              else setProblem(result.failure.message);
            }}
          >
            {pending ? "Deleting…" : "Delete"}
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
