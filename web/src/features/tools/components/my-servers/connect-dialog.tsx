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
import type { ApiUserToolAuthorization, ApiUserToolServer } from "@/lib/api/types";
import { openInBrowser, usesNativeBrowser } from "@/lib/auth/desktop";
import { hostOf } from "../../lib/user-servers";

type Props = {
  server: ApiUserToolServer;
  /** The answer of `authorizeUserToolServer`. It is a capability for the length of the flow: held by this dialog alone. */
  flow: ApiUserToolAuthorization;
  /** The person chose to go on: the window is opened (by this dialog) and the row waits for them to come back. */
  onOpened: () => void;
  onClose: () => void;
};

/** How long the link lasts, in words that do not depend on the clock: "about 10 minutes". */
const lasts = (iso: string): string => {
  const ms = new Date(iso).getTime() - Date.now();
  if (Number.isNaN(ms)) return "";
  const minutes = Math.round(ms / 60_000);
  return minutes >= 2 ? `about ${minutes} minutes` : "less than 2 minutes";
};

/**
 * **Connect** (ADR 0046, slice 4c): the server a stranger added chose its own authorization server, and the person is
 * about to be sent there, so the screen says where, by its host, **before** anything opens. `authorizeUrl` is opened
 * only by the button, in a window of its own (no opener, no referrer), and is never drawn as text or a link, never
 * logged and not kept after the dialog. A flow that has ended (`expiresAt`) cannot be continued; the person connects again.
 */
export function ConnectDialog({ server, flow, onOpened, onClose }: Props) {
  const host = hostOf(flow.issuer);
  const validFor = lasts(flow.expiresAt);
  const [problem, setProblem] = useState<string | null>(null);
  const ended = new Date(flow.expiresAt).getTime() <= Date.now();
  return (
    <AlertDialog
      open
      onOpenChange={(open) => {
        if (!open) onClose();
      }}
    >
      <AlertDialogContent data-slot="connect-dialog">
        <AlertDialogHeader>
          <AlertDialogTitle>Connect “{server.name}”?</AlertDialogTitle>
          <AlertDialogDescription asChild>
            <div className="flex flex-col gap-2">
              <p>
                You will be sent to <strong className="font-medium text-foreground">{host}</strong>{" "}
                to sign in. This sign-in service was chosen by the server you added, not by this
                deployment. Continue only if you trust it.
              </p>
              <p data-slot="issuer" className="font-mono text-xs break-all text-foreground">
                {flow.issuer}
              </p>
              <p>
                {ended
                  ? "This link has ended. Close this and connect again."
                  : validFor
                    ? `It opens in a new window and works for ${validFor}.`
                    : "It opens in a new window."}
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
          <AlertDialogCancel>Cancel</AlertDialogCancel>
          <AlertDialogAction
            disabled={ended}
            onClick={async (event) => {
              // the apps open the system browser, which their native side may refuse for an address that is not their own
              // issuer's: the dialog then says so and stays, rather than going quiet
              if (usesNativeBrowser()) {
                event.preventDefault();
                try {
                  await openInBrowser(flow.authorizeUrl);
                  onOpened();
                } catch {
                  setProblem(
                    "This app could not open the sign-in in your browser. Connect this server from the web app.",
                  );
                }
                return;
              }
              window.open(flow.authorizeUrl, "_blank", "noopener,noreferrer");
              onOpened();
            }}
          >
            Continue to {host}
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
