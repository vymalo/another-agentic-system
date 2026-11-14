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
import type { ApiUserToolServer } from "@/lib/api/types";
import type { WriteResult } from "../../hooks/use-user-tool-servers";

type Props = {
  server: ApiUserToolServer;
  /** `deleteUserToolServer`: no revision, and no permission is needed. */
  onDelete: (id: string) => Promise<WriteResult<null>>;
  onClose: () => void;
};

/**
 * Deleting forgets the server's credential first (ADR 0046): the dialog says what goes before anything is sent, and
 * says what came of it when the server could not be deleted (the credential could not be revoked: nothing was lost,
 * the server is still there). A chat that had the server attached keeps its id and simply cannot reach it any more.
 */
export function DeleteServerDialog({ server, onDelete, onClose }: Props) {
  const [pending, setPending] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  return (
    <AlertDialog
      open
      onOpenChange={(open) => {
        if (!open && !pending) onClose();
      }}
    >
      <AlertDialogContent data-slot="delete-server-dialog">
        <AlertDialogHeader>
          <AlertDialogTitle>Delete “{server.name}”?</AlertDialogTitle>
          <AlertDialogDescription asChild>
            <div className="flex flex-col gap-2">
              <p>
                This removes the server from your list
                {server.hasCredential ? " and forgets its key or sign-in" : ""}. Chats that had it
                attached keep the name but can no longer reach it. This cannot be undone.
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
              // the dialog stays open until it is known whether the server is gone
              event.preventDefault();
              setPending(true);
              setProblem(null);
              const result = await onDelete(server.id);
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
