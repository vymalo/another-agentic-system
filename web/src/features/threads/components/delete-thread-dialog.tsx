"use client";

import { Loader2Icon } from "lucide-react";
import type { ComponentProps } from "react";
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
import {
  ERASE_TIMING,
  type EraseTiming,
  useEraseThread,
} from "@/features/threads/hooks/use-erase-thread";
import type { ApiThread } from "@/lib/api/types";

/**
 * Deleting a thread erases it (ADR 0043), so the dialog says what goes and what stays, in plain words,
 * before anything is sent. A thread that works cannot be deleted: its dialog says so and offers
 * "Stop and delete", which stops the agent, waits for it to report the stop and deletes. Mounted for one
 * thread (give it a `key`); `onErased` is told when the chat is gone, by this delete or already.
 */
export function DeleteThreadDialog({
  thread,
  onClose,
  onErased,
  timing = ERASE_TIMING,
  onCloseAutoFocus,
}: {
  thread: ApiThread;
  onClose: () => void;
  onErased: (id: string) => void;
  timing?: EraseTiming;
  onCloseAutoFocus?: ComponentProps<typeof AlertDialogContent>["onCloseAutoFocus"];
}) {
  const eraser = useEraseThread(thread, { onErased, timing });
  const { state, needsStop } = eraser;
  const busy = state.phase === "erasing" || state.phase === "stopping";
  const title = thread.title || "Untitled";
  return (
    <AlertDialog
      open
      onOpenChange={(open) => {
        if (open) return;
        eraser.abort();
        onClose();
      }}
    >
      <AlertDialogContent onCloseAutoFocus={onCloseAutoFocus} data-slot="delete-thread-dialog">
        <AlertDialogHeader>
          <AlertDialogTitle>Delete “{title}”?</AlertDialogTitle>
          <AlertDialogDescription asChild>
            <div className="flex flex-col gap-2">
              <p>
                {needsStop
                  ? "This chat is still working. The agent has to stop before it can be erased."
                  : "This erases the chat for good."}
              </p>
              <p className="text-foreground">Deleting it erases:</p>
              <ul className="ms-5 list-disc space-y-0.5">
                <li>the conversation, with its steps</li>
                <li>the chats made from it by editing a message</li>
                <li>the files the agents made in it</li>
                {thread.share ? <li>its share link, which stops working</li> : null}
              </ul>
              <p>Forks of it are kept, as chats of their own. This cannot be undone.</p>
            </div>
          </AlertDialogDescription>
        </AlertDialogHeader>
        {state.phase === "stopping" ? (
          <InlineStatus role="status">
            <Loader2Icon
              aria-hidden="true"
              className="me-1.5 inline size-3.5 motion-safe:animate-spin"
            />
            Stopping the agent. This can take a few seconds.
          </InlineStatus>
        ) : null}
        {state.phase === "failed" ? (
          <InlineStatus tone="error" role="alert">
            {state.message}
          </InlineStatus>
        ) : null}
        <AlertDialogFooter>
          <AlertDialogCancel>Cancel</AlertDialogCancel>
          <AlertDialogAction
            disabled={busy}
            className={buttonVariants({ variant: "destructive" })}
            onClick={(event) => {
              // the dialog stays open: it says what came of it
              event.preventDefault();
              if (needsStop) eraser.stopAndErase();
              else eraser.erase();
            }}
          >
            {state.phase === "erasing"
              ? "Deleting…"
              : state.phase === "stopping"
                ? "Stopping…"
                : needsStop
                  ? "Stop and delete"
                  : "Delete"}
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
