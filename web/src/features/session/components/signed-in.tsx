"use client";

import { useEffect, useState } from "react";
import { SIGNED_IN_CHANNEL } from "@/lib/api/session-refresh";

/**
 * The last page of the sign-in popup (`/signed-in`, the `rd` of `openSignIn`). Reaching it means the
 * edge let the request through, so there is a session: it tells the tabs that are waiting (they
 * check with the edge, they do not take its word) and closes the window. A window that was not
 * opened by a script cannot close itself, and then says so, with a link to the app.
 */
export function SignedIn() {
  const [open, setOpen] = useState(false);
  useEffect(() => {
    try {
      const channel = new BroadcastChannel(SIGNED_IN_CHANNEL);
      channel.postMessage("signed-in");
      channel.close();
    } catch {
      // no channel: the waiting page asks again when its window is back
    }
    window.close();
    // still here: not a popup
    const timer = setTimeout(() => setOpen(true), 300);
    return () => clearTimeout(timer);
  }, []);
  return (
    <main className="flex min-h-dvh items-center justify-center p-6">
      <div className="max-w-sm text-center">
        <h1 className="text-lg font-medium">You are signed in</h1>
        <p className="mt-1 text-sm text-muted-foreground">
          {open ? "You can close this window and go back to the chat." : "Closing this window…"}
        </p>
        {open ? (
          <a href="/" className="mt-3 inline-block text-sm underline underline-offset-3">
            Open the chat
          </a>
        ) : null}
      </div>
    </main>
  );
}
