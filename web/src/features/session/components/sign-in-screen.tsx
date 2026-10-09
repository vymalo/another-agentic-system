"use client";

import { LogInIcon } from "lucide-react";
import { type ReactNode, useEffect, useState, useSyncExternalStore } from "react";
import { PandaMark } from "@/components/brand/panda-mark";
import { InlineStatus } from "@/components/inline-status";
import { Button } from "@/components/ui/button";
import { hereAsReturnTo, startSignIn } from "@/lib/auth/sign-in";
import {
  requireSignIn,
  type SignInNeed,
  signInNeed,
  subscribeSignInNeed,
} from "@/lib/auth/sign-in-need";
import { storedSession } from "@/lib/auth/tokens";
import type { BrowserAuthConfig } from "@/lib/auth/types";
import { useBrowserAuth } from "@/lib/auth/use-browser-auth";

/** The words, one place: the screen and the tests read them. */
export const SIGN_IN_LABEL = "Sign in";
export const SESSION_ENDED_LINE = "Your session has ended. Sign in again to go on.";
export const ISSUER_UNREACHABLE =
  "The sign-in service could not be reached. Try again in a moment.";

/**
 * Who the person signs in with, from the issuer's address: a Keycloak realm (`/realms/<name>`) is the
 * organisation, else the issuer's host; and the host, which the person sees in the address bar next.
 */
export function organisationOf(issuer: string): { name: string; host: string } {
  try {
    const url = new URL(issuer);
    const realm = /\/realms\/([^/]+)\/?$/.exec(url.pathname)?.[1];
    return { name: realm ? decodeURIComponent(realm) : url.hostname, host: url.hostname };
  } catch {
    return { name: issuer, host: "" };
  }
}

/**
 * The app's own sign-in screen (browser mode, ADR 0054): the panda, one **Sign in** button and a line that
 * names the organisation. Pressing the button is the one way to the issuer (authorization code with PKCE,
 * back to this page). `ended`: the person was signed in on this browser and the issuer has refused it since.
 */
export function SignInScreen({
  config,
  reason,
}: {
  config: BrowserAuthConfig;
  reason: SignInNeed;
}) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const org = organisationOf(config.issuer);
  const signIn = () => {
    setBusy(true);
    setError(null);
    startSignIn({ returnTo: hereAsReturnTo() }).catch(() => {
      setBusy(false);
      setError(ISSUER_UNREACHABLE);
    });
  };
  return (
    <main
      data-slot="sign-in"
      className="flex min-h-dvh flex-col items-center justify-center gap-5 px-6 py-12 text-center"
    >
      <PandaMark size={72} />
      <h1 className="text-[1.75rem] leading-tight font-semibold tracking-tight">
        another<span className="text-brand">·</span>agentic
      </h1>
      {reason === "ended" ? (
        <p role="status" className="max-w-md text-[0.9375rem] text-balance">
          {SESSION_ENDED_LINE}
        </p>
      ) : null}
      <p className="max-w-md text-sm text-muted-foreground text-balance">
        {org.name === org.host ? (
          <>
            Sign in at <strong className="font-medium text-foreground">{org.host}</strong>.
          </>
        ) : (
          <>
            Sign in with your <strong className="font-medium text-foreground">{org.name}</strong>{" "}
            account{org.host ? ` at ${org.host}` : ""}.
          </>
        )}
      </p>
      <Button
        type="button"
        size="lg"
        className="min-w-40 rounded-full"
        disabled={busy}
        onClick={signIn}
      >
        <LogInIcon aria-hidden="true" />
        {SIGN_IN_LABEL}
      </Button>
      {error ? (
        <InlineStatus tone="error" role="alert">
          {error}
        </InlineStatus>
      ) : null}
    </main>
  );
}

/**
 * Stands in for the app with the sign-in screen when it needs a sign-in (browser mode only; with an
 * edge, the edge signs people in before the page loads, and this renders the app as it always did).
 * `check`: on a page of the app, it asks this browser first whether it holds a usable sign-in (without
 * creating anything); a share link's page does not (a public reader is never asked to sign in), and shows
 * the screen only when the link turns out to need one.
 */
export function SignInGate({ children, check = true }: { children: ReactNode; check?: boolean }) {
  const cfg = useBrowserAuth();
  const need = useSyncExternalStore(subscribeSignInNeed, signInNeed, () => null);
  useEffect(() => {
    if (!cfg || !check) return;
    let live = true;
    void storedSession().then(
      (held) => {
        if (live && held !== "usable") requireSignIn(held);
      },
      () => {},
    );
    return () => {
      live = false;
    };
  }, [cfg, check]);
  if (cfg && need) return <SignInScreen config={cfg} reason={need} />;
  return children;
}
