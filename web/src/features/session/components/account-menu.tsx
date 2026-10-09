"use client";

import { ChevronsUpDownIcon, LogOutIcon } from "lucide-react";
import { useState } from "react";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { useMe } from "@/features/me/hooks/use-me";
import { canSignOut, signOutNow } from "@/lib/api/session";
import { useBrowserAuth } from "@/lib/auth/use-browser-auth";

/** The words, one place: the menu, the no-access screen and the tests read them. */
export const SIGN_OUT_LABEL = "Sign out";

/** Whether this deployment has a sign-in to end; it is known once the page has asked (`useBrowserAuth`). */
export function useCanSignOut(): boolean {
  const cfg = useBrowserAuth();
  return cfg !== undefined && canSignOut();
}

/**
 * The person at the foot of the sidebar: who they are (`GET /api/me`) and a menu with **Sign out**. In
 * browser mode it revokes the refresh token, deletes this browser's key and session and ends the
 * issuer's session (ADR 0054, decision 11); with an edge it goes to oauth2-proxy's sign-out. Nothing
 * when there is neither a person to show nor a sign-in to end (a local run without an edge).
 */
export function AccountMenu() {
  const me = useMe();
  const signOut = useCanSignOut();
  const [leaving, setLeaving] = useState(false);
  const person = me.status === "ready" ? me.me : null;
  // while `/api/me` is on its way: nothing, rather than a placeholder that turns into the person
  if (me.status === "loading" || (!person && !signOut)) return null;
  const email = person?.email ?? person?.user;
  const title = person?.name ?? email ?? "Your account";
  const initial = (title.trim()[0] ?? "?").toUpperCase();
  return (
    <div data-slot="account" className="shrink-0 border-t px-2 py-2">
      <DropdownMenu>
        <Tooltip>
          <TooltipTrigger asChild>
            <DropdownMenuTrigger asChild>
              <Button
                type="button"
                variant="ghost"
                aria-label={`Account: ${email ?? title}`}
                className="h-12 w-full justify-start gap-2.5 rounded-xl px-2 text-start hover:bg-sidebar-accent/70"
              >
                <span
                  aria-hidden="true"
                  className="flex size-8 shrink-0 items-center justify-center rounded-full bg-brand/15 text-sm font-medium text-foreground"
                >
                  {initial}
                </span>
                <span className="flex min-w-0 flex-1 flex-col">
                  <span className="truncate text-sm font-medium text-foreground">{title}</span>
                  {email && email !== title ? (
                    <span className="truncate text-xs text-muted-foreground">{email}</span>
                  ) : null}
                </span>
                <ChevronsUpDownIcon aria-hidden="true" className="size-4 text-muted-foreground" />
              </Button>
            </DropdownMenuTrigger>
          </TooltipTrigger>
          <TooltipContent side="top">
            {email ? `Signed in as ${email}` : "Your account"}
          </TooltipContent>
        </Tooltip>
        <DropdownMenuContent
          side="top"
          align="start"
          className="w-(--radix-dropdown-menu-trigger-width)"
        >
          {email ? (
            <>
              <DropdownMenuLabel className="truncate font-normal">
                Signed in as <span className="font-medium text-foreground">{email}</span>
              </DropdownMenuLabel>
              {signOut ? <DropdownMenuSeparator /> : null}
            </>
          ) : null}
          {signOut ? (
            <DropdownMenuItem
              disabled={leaving}
              onSelect={() => {
                if (signOutNow() !== "none") setLeaving(true);
              }}
            >
              <LogOutIcon aria-hidden="true" />
              {SIGN_OUT_LABEL}
            </DropdownMenuItem>
          ) : null}
        </DropdownMenuContent>
      </DropdownMenu>
    </div>
  );
}

/** **Sign out** as a button of its own (the no-access screen: "signed in with the wrong account"). Nothing where there is no sign-in to end. */
export function SignOutButton() {
  const signOut = useCanSignOut();
  const [leaving, setLeaving] = useState(false);
  if (!signOut) return null;
  return (
    <Button
      type="button"
      variant="outline"
      disabled={leaving}
      onClick={() => {
        if (signOutNow() !== "none") setLeaving(true);
      }}
    >
      <LogOutIcon aria-hidden="true" />
      {SIGN_OUT_LABEL}
    </Button>
  );
}
