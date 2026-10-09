"use client";

import * as oauth from "oauth4webapi";
import { useEffect, useState } from "react";
import { Button } from "@/components/ui/button";
import { navigation } from "@/lib/auth/navigation";
import { announceSignedIn, type Completed, completeSignIn, startSignIn } from "@/lib/auth/sign-in";

/**
 * The page the issuer sends the browser back to (`/auth/callback`, ADR 0054): it exchanges the code
 * (with a DPoP proof), stores the session and goes on. A redirect sign-in returns to the page the
 * person was on; the popup of "Sign in again" tells the tabs that wait, over the same channel as
 * the edge's `/signed-in`, and closes itself. A window that was not opened by a script cannot close
 * itself, and says so, with a link to the app.
 */

// React runs an effect twice in development, and a code can be exchanged once: one exchange per page
let exchange: Promise<Completed> | undefined;
const exchangeOnce = () => {
  exchange ??= completeSignIn(window.location.href);
  return exchange;
};

type Phase =
  | { name: "working" }
  | { name: "popup"; stuck: boolean }
  | { name: "failed"; message: string };

/** The issuer's own words when it refused (an `error` on the callback, or at the token endpoint), else the error's. */
const words = (e: unknown): string => {
  if (e instanceof oauth.AuthorizationResponseError || e instanceof oauth.ResponseBodyError) {
    return `The issuer said: ${e.error_description || e.error}`;
  }
  return e instanceof Error && e.message ? e.message : "The sign-in did not finish.";
};

export function AuthCallback() {
  const [phase, setPhase] = useState<Phase>({ name: "working" });

  useEffect(() => {
    let timer: ReturnType<typeof setTimeout> | undefined;
    let live = true;
    exchangeOnce().then(
      (done) => {
        // the code and the state are spent: the address is of no use any more
        window.history.replaceState(null, "", window.location.pathname);
        if (done.mode === "redirect") return navigation.replace(done.returnTo);
        announceSignedIn();
        window.close();
        if (!live) return;
        setPhase({ name: "popup", stuck: false });
        // still here: not a window a script opened
        timer = setTimeout(() => live && setPhase({ name: "popup", stuck: true }), 300);
      },
      (e: unknown) => live && setPhase({ name: "failed", message: words(e) }),
    );
    return () => {
      live = false;
      clearTimeout(timer);
    };
  }, []);

  return (
    <main className="flex min-h-dvh items-center justify-center p-6">
      <div className="max-w-sm text-center" role="status" aria-live="polite">
        {phase.name === "failed" ? (
          <>
            <h1 className="text-lg font-medium">Signing in did not work</h1>
            <p className="mt-1 text-sm text-muted-foreground [overflow-wrap:anywhere]">
              {phase.message}
            </p>
            <Button
              type="button"
              className="mt-4"
              onClick={() => void startSignIn({ returnTo: "/" }).catch(() => {})}
            >
              Try again
            </Button>
          </>
        ) : (
          <>
            <h1 className="text-lg font-medium">
              {phase.name === "popup" ? "You are signed in" : "Signing you in"}
            </h1>
            <p className="mt-1 text-sm text-muted-foreground">
              {phase.name === "popup"
                ? phase.stuck
                  ? "You can close this window and go back to the chat."
                  : "Closing this window…"
                : "One moment…"}
            </p>
            {phase.name === "popup" && phase.stuck ? (
              <a href="/" className="mt-3 inline-block text-sm underline underline-offset-3">
                Open the chat
              </a>
            ) : null}
          </>
        )}
      </div>
    </main>
  );
}
