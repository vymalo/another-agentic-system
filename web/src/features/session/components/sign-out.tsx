"use client";

import { LogOutIcon } from "lucide-react";
import { useState } from "react";
import { Button } from "@/components/ui/button";
import { edgeSignOutUrl } from "@/lib/api/session";
import { signOut } from "@/lib/auth/sign-out";
import { useBrowserAuth } from "@/lib/auth/use-browser-auth";

/**
 * Signing out (`/auth/sign-out`, ADR 0054, decision 11): the refresh token is revoked at the issuer,
 * the keys and the session are deleted from this browser, and the issuer ends its own session.
 * A confirmation, not a bare link: a page that signs a person out when it is opened could be opened
 * by any other page. An edge deployment signs out at its edge (oauth2-proxy's `/sign_out`). The
 * app's own control is the account menu (`account-menu.tsx`), which signs out on its click.
 */
export function SignOut() {
  const cfg = useBrowserAuth();
  const [busy, setBusy] = useState(false);
  const edge = edgeSignOutUrl();
  return (
    <main className="flex min-h-dvh items-center justify-center p-6">
      <div className="max-w-sm text-center">
        <h1 className="text-lg font-medium">Sign out</h1>
        {cfg ? (
          <>
            <p className="mt-1 text-sm text-muted-foreground">
              This ends your sign-in on this device and at the issuer.
            </p>
            <Button
              type="button"
              className="mt-4"
              disabled={busy}
              onClick={() => {
                setBusy(true);
                void signOut();
              }}
            >
              <LogOutIcon aria-hidden="true" />
              Sign out
            </Button>
          </>
        ) : cfg === null && edge ? (
          <a href={edge} className="mt-3 inline-block text-sm underline underline-offset-3">
            Sign out at the edge
          </a>
        ) : cfg === null ? (
          <p className="mt-1 text-sm text-muted-foreground">There is no sign-in to end here.</p>
        ) : (
          <p className="mt-1 text-sm text-muted-foreground">One moment…</p>
        )}
      </div>
    </main>
  );
}
