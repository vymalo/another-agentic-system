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
import type { ApiUserToolAuthorization, ApiUserToolServer } from "@/lib/api/types";
import { openInBrowser, usesNativeBrowser } from "@/lib/auth/desktop";
import { openableLink } from "../../lib/user-servers";

type Props = {
  server: ApiUserToolServer;
  /** The answer of `authorizeUserToolServer`. It is a capability for the length of the flow: held by this dialog alone. */
  flow: ApiUserToolAuthorization;
  /** The person chose to go on: the row waits for them to come back. */
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
 * about to be sent there, so the screen says where **before** anything opens. What it names is the host of the link
 * itself (`openableLink`: the orchestrator takes the authorization endpoint from the server's metadata and need not put
 * it on the issuer's host), beside the issuer's when they differ. A link that is not `https` (or `http` on this machine)
 * is never opened: the dialog says the server gave a link that cannot be opened. In a browser the way on is a real link
 * (`target="_blank"`, no opener, no referrer), so a pop-up blocker has nothing to block and nothing claims a window that
 * may not exist; in the apps it is the system browser through the native side. The URL lives in this dialog alone and is gone
 * with it; a link that has ended (`expiresAt`) cannot be continued.
 */
export function ConnectDialog({ server, flow, onOpened, onClose }: Props) {
  const link = openableLink(flow.authorizeUrl, flow.issuer);
  const validFor = lasts(flow.expiresAt);
  const [problem, setProblem] = useState<string | null>(null);
  const ended = new Date(flow.expiresAt).getTime() <= Date.now();
  const native = usesNativeBrowser();
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
              {link.ok ? (
                <>
                  <p>
                    You will be sent to{" "}
                    <strong data-slot="destination" className="font-medium text-foreground">
                      {link.host}
                    </strong>{" "}
                    to sign in. This sign-in service was chosen by the server you added, not by this
                    deployment. Continue only if you trust it.
                  </p>
                  {link.sameHost ? null : (
                    <p data-slot="hosts">
                      The server names <strong className="font-medium">{link.issuerHost}</strong> as
                      its authorization server, and the sign-in page is on{" "}
                      <strong className="font-medium">{link.host}</strong>.
                    </p>
                  )}
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
                </>
              ) : (
                <p data-slot="unopenable">
                  The server gave a sign-in link that cannot be opened, so nothing was opened. Do
                  not connect this server; delete it if you do not know it.
                </p>
              )}
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
          {link.ok && !ended && !native ? (
            <AlertDialogAction asChild>
              <a
                href={link.href}
                target="_blank"
                rel="noopener noreferrer"
                className={`${buttonVariants({ variant: "default" })} no-underline`}
                onClick={onOpened}
              >
                Continue to {link.host}
              </a>
            </AlertDialogAction>
          ) : (
            <AlertDialogAction
              disabled={!link.ok || ended}
              onClick={async (event) => {
                if (!link.ok) return;
                // the apps open the system browser, which their native side may refuse for an address that is not their own
                // issuer's: the dialog then says so and stays, rather than going quiet
                event.preventDefault();
                try {
                  await openInBrowser(link.href);
                  onOpened();
                } catch {
                  setProblem(
                    "This app could not open the sign-in in your browser. Connect this server from the web app.",
                  );
                }
              }}
            >
              {link.ok ? `Continue to ${link.host}` : "Continue"}
            </AlertDialogAction>
          )}
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
