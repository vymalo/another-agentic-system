"use client";

import { RefreshCwIcon } from "lucide-react";
import { useSyncExternalStore } from "react";
import { PandaMark } from "@/components/brand/panda-mark";
import { ThemeMenu } from "@/components/theme/theme-menu";
import { Button } from "@/components/ui/button";
import { subscribeUpdateRequired, updateRequired } from "@/lib/api/client-version";

/** The words, one place: the screen and the tests read them. */
export const UPDATE_REQUIRED_TITLE = "Update the application";
export const UPDATE_REQUIRED_TEXT =
  "This version of the application is too old for the service it talks to. Update it to go on.";
export const UPDATE_REQUIRED_ACTION = "Reload";

/**
 * What takes the page when the deployment refuses this version of the application (`lib/api/client-version.ts`: the
 * problem `client_update_required`): a full-page state in place of an error line, with one way forward. On the web an
 * update is a new load of the page; the desktop and phone apps are updated where they were installed, and the same
 * screen tells the person to. Nothing while the version is accepted.
 */
export function UpdateRequiredScreen() {
  const required = useSyncExternalStore(subscribeUpdateRequired, updateRequired, () => false);
  if (!required) return null;
  return (
    <main
      data-slot="update-required"
      aria-labelledby="update-required-title"
      className="fixed inset-0 z-[100] flex flex-col items-center justify-center gap-5 overflow-y-auto bg-background px-6 py-12 text-center"
    >
      <div className="absolute end-3 top-3">
        <ThemeMenu />
      </div>
      <PandaMark size={72} />
      <h1
        id="update-required-title"
        className="text-[1.75rem] leading-tight font-semibold tracking-tight"
      >
        {UPDATE_REQUIRED_TITLE}
      </h1>
      <p className="max-w-sm [overflow-wrap:anywhere] text-muted-foreground">
        {UPDATE_REQUIRED_TEXT}
      </p>
      <Button type="button" onClick={() => window.location.reload()}>
        <RefreshCwIcon aria-hidden="true" />
        {UPDATE_REQUIRED_ACTION}
      </Button>
    </main>
  );
}
