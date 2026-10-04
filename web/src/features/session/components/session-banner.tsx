"use client";

import { LogInIcon } from "lucide-react";
import { useEffect, useState, useSyncExternalStore } from "react";
import { Button } from "@/components/ui/button";
import { openSignIn } from "@/lib/api/session";
import {
  endedAgainSoon,
  sessionStatus,
  subscribeSession,
  watchForSignIn,
} from "@/lib/api/session-refresh";

/** The words, one place: the banner and the tests read them. */
export const SESSION_ENDED_TITLE = "Your session has ended";
export const SESSION_ENDED_TEXT =
  "Sign in again to go on. This page stays as it is: what was waiting for you is sent once you are back.";
export const SIGN_IN_PAUSED_TEXT =
  "Signing in did not help a moment ago. Wait a little and try again, or reload the page.";

/**
 * What the person sees when the edge has no session for them and a refresh could not make one
 * (`lib/api/session-refresh.ts`): one line over the page with one button. The button opens the sign-in
 * in a popup that closes itself, and the page, its stream and the message in the box stay where they
 * are; the requests that were held go on when the session is back, which the page notices by itself
 * (the popup says so, a window that comes back asks). It is a `status`, not an `alert`: the page did
 * not fail, and a `role="alert"` is how the page's own error lines are found. Nothing when there is a
 * session, and never on a page without the edge's sign-in (a request is then a plain 401).
 */
export function SessionBanner() {
  const status = useSyncExternalStore(subscribeSession, sessionStatus, () => "ok" as const);
  const ended = status === "ended";
  const [paused, setPaused] = useState(false);
  // ended again right after a sign-in: it did not help, whether or not a redirect was held back
  const [again, setAgain] = useState(false);

  useEffect(() => {
    if (!ended) {
      setPaused(false);
      return;
    }
    setAgain(endedAgainSoon());
    return watchForSignIn();
  }, [ended]);

  if (!ended) return null;
  return (
    <div
      role="status"
      data-slot="session-banner"
      className="fixed inset-x-4 top-3 z-50 mx-auto flex max-w-xl flex-col gap-2 rounded-2xl border border-warning/40 bg-warning-soft px-4 py-3 text-sm text-foreground shadow-md sm:flex-row sm:items-center sm:gap-4"
    >
      <div className="min-w-0 flex-1">
        <p className="font-medium">{SESSION_ENDED_TITLE}</p>
        <p className="[overflow-wrap:anywhere]">
          {paused || again ? SIGN_IN_PAUSED_TEXT : SESSION_ENDED_TEXT}
        </p>
      </div>
      <Button
        type="button"
        size="sm"
        className="shrink-0"
        onClick={() => setPaused(openSignIn() === "paused")}
      >
        <LogInIcon aria-hidden="true" />
        Sign in
      </Button>
    </div>
  );
}
