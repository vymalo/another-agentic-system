"use client";

import NotFound from "@/app/not-found";
import { ChatShell } from "@/features/chat/components/chat-shell";
import { SignInGate } from "@/features/session/components/sign-in-screen";
import { useAddressSegment } from "./address";

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

/** `/threads/<id>`: the chat of that thread, or the page of an address that is not one (ADR 0047). */
export function ThreadRoute() {
  const id = useAddressSegment("/threads/");
  if (id === undefined) return null;
  if (id === null || !UUID.test(id)) return <NotFound />;
  return (
    <SignInGate>
      <ChatShell key={id} threadId={id} />
    </SignInGate>
  );
}
