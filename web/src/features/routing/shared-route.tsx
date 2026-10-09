"use client";

import { SignInGate } from "@/features/session/components/sign-in-screen";
import { SharedChat } from "@/features/sharing/components/shared-chat";
import { useAddressSegment } from "./address";

/**
 * `/s/<token>` (ADR 0040, ADR 0047). The token is not checked here: a token that cannot be one is the same
 * page as any link that does not work, which `SharedChat` says. A link that is not public, read by nobody
 * signed in, is the sign-in screen (browser mode); a public reader never sees it.
 */
export function SharedRoute() {
  const token = useAddressSegment("/s/");
  if (token === undefined) return null;
  return (
    <SignInGate check={false}>
      <SharedChat key={token ?? ""} token={token ?? ""} />
    </SignInGate>
  );
}
