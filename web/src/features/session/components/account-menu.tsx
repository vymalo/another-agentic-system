"use client";

import { ChartNoAxesColumnIcon, ChevronsUpDownIcon, LogOutIcon, WrenchIcon } from "lucide-react";
import Link from "next/link";
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
import { holds } from "@/features/me/lib/access";
import { canSignOut, signOutNow } from "@/lib/api/session";
import { useBrowserAuth } from "@/lib/auth/use-browser-auth";

/** The words, one place: the menu, the no-access screen and the tests read them. */
export const SIGN_OUT_LABEL = "Sign out";
export const USAGE_LABEL = "Usage";
export const TOOL_SERVERS_LABEL = "My tool servers";

/** Whether this deployment has a sign-in to end; it is known once the page has asked (`useBrowserAuth`). */
export function useCanSignOut(): boolean {
  const cfg = useBrowserAuth();
  return cfg !== undefined && canSignOut();
}

/**
 * The person at the foot of the sidebar: who they are (`GET /api/me`) and a menu with **Usage** (the page of what they used this month, ADR 0065),
 * **My tool servers** (the MCP servers they add for themselves, ADR 0046; only for a person whose roles hold `tools.user.add`, as `GET /api/me`
 * says; the page answers for itself when it is opened by its address without it) and **Sign out**. In
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
              <DropdownMenuSeparator />
            </>
          ) : null}
          {/* no permission gates it: the route answers for the person who asks, and for nobody else (ADR 0065) */}
          <DropdownMenuItem asChild>
            <Link href="/usage" className="text-foreground no-underline">
              <ChartNoAxesColumnIcon aria-hidden="true" />
              {USAGE_LABEL}
            </Link>
          </DropdownMenuItem>
          {person && holds(person, "tools.user.add") ? (
            <DropdownMenuItem asChild>
              <Link href="/tools" className="text-foreground no-underline">
                <WrenchIcon aria-hidden="true" />
                {TOOL_SERVERS_LABEL}
              </Link>
            </DropdownMenuItem>
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
