"use client";

import { KeyRoundIcon } from "lucide-react";
import Link from "next/link";
import type { ReactNode } from "react";
import { ActorLabel } from "@/features/chat/components/actor-label";
import type { ApiActor } from "@/lib/api/types";
import { TOOLS_HREF } from "../lib/receipt";

/*
 * The client's own chrome of a trusted component (ADR 0083, decision 2): the mark, the words and the sentence that the
 * agent cannot see what is typed. It is drawn from the `vymalo.trustedRequest` activity, never from a surface, so no
 * surface — nothing an agent can draw — can reproduce it.
 */

/** The card's frame: the mark, the sentence the agent cannot cross, and who asked. */
export function TrustedFrame({
  actor,
  title,
  children,
}: {
  actor: ApiActor | undefined;
  title: string;
  children: ReactNode;
}) {
  return (
    <section
      data-slot="trusted-request"
      aria-label={`Trusted request from ${actor?.name ?? "the agent"}`}
      className="flex w-full max-w-xl min-w-0 flex-col gap-3 rounded-xl border bg-card p-3.5 text-card-foreground"
    >
      <header className="flex min-w-0 items-start gap-2.5">
        <span
          aria-hidden="true"
          className="flex size-8 shrink-0 items-center justify-center rounded-lg bg-muted text-muted-foreground"
        >
          <KeyRoundIcon className="size-4" />
        </span>
        <div className="flex min-w-0 flex-1 flex-col gap-0.5">
          <p className="text-sm font-medium">{title}</p>
          <p data-slot="trusted-blind" className="text-[0.8125rem] text-muted-foreground">
            The agent can’t see this. You type it here, and this app saves it for you.
          </p>
        </div>
        <ActorLabel actor={actor} />
      </header>
      {children}
    </section>
  );
}

/** The words of a card this build does not draw, or cannot draw here, with the screen that does it. */
export function TrustedFallback({ children }: { children: ReactNode }) {
  return (
    <div className="flex flex-col gap-1.5 text-[0.8125rem] [overflow-wrap:anywhere]">
      {children}
      <p>
        You can add a tool server yourself on{" "}
        <Link
          href={TOOLS_HREF}
          className="underline underline-offset-4 hover:no-underline focus-visible:outline-2 focus-visible:outline-ring"
        >
          My tool servers
        </Link>
        .
      </p>
    </div>
  );
}

/** A muted line of the card: something the agent suggested, said in the client's words. */
export function TrustedNote({ children }: { children: ReactNode }) {
  return <p className="text-xs text-muted-foreground [overflow-wrap:anywhere]">{children}</p>;
}
