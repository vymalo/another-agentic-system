"use client";

import { RefreshCwIcon } from "lucide-react";
import { type ReactNode, useSyncExternalStore } from "react";
import { PandaMark } from "@/components/brand/panda-mark";
import { ThemeMenu } from "@/components/theme/theme-menu";
import { Button } from "@/components/ui/button";
import { subscribeUpdateNeed, updateNeed } from "@/lib/api/client-version";

/** The words, one place: the screen and the tests read them. */
export const UPDATE_TITLE = "Update the application";
export const UPDATE_TEXT =
  "This version is too old to work with this deployment. Update it, then open it again.";
export const RELOAD_LABEL = "Reload";

/** The full-page "update the application" state: the panda, what is needed and a Reload button. */
export function UpdateScreen({ minClientVersion }: { minClientVersion?: string }) {
  return (
    <main
      data-slot="update-required"
      className="relative flex min-h-dvh flex-col items-center justify-center gap-5 px-6 py-12 text-center"
    >
      <div className="absolute end-3 top-3">
        <ThemeMenu />
      </div>
      <PandaMark size={72} />
      <h1 className="text-[1.75rem] leading-tight font-semibold tracking-tight">{UPDATE_TITLE}</h1>
      <p role="status" className="max-w-md text-[0.9375rem] text-balance">
        {UPDATE_TEXT}
      </p>
      {minClientVersion ? (
        <p className="text-sm text-muted-foreground">
          Needs version <strong className="font-medium text-foreground">{minClientVersion}</strong>{" "}
          or later.
        </p>
      ) : null}
      <Button
        type="button"
        size="lg"
        className="min-w-40 rounded-full"
        onClick={() => window.location.reload()}
      >
        <RefreshCwIcon aria-hidden="true" />
        {RELOAD_LABEL}
      </Button>
    </main>
  );
}

/** Stands in for the whole app with the update screen once the deployment has refused this version. */
export function UpdateGate({ children }: { children: ReactNode }) {
  const need = useSyncExternalStore(subscribeUpdateNeed, updateNeed, () => null);
  if (need)
    return (
      <UpdateScreen
        {...(need.minClientVersion ? { minClientVersion: need.minClientVersion } : {})}
      />
    );
  return children;
}
