"use client";

import type { ReactNode } from "react";
import { parseTrustedRequest } from "@/features/chat/lib/agui/vymalo";
import { isDrawn, shownName } from "../lib/receipt";
import { TrustedFallback, TrustedFrame } from "./chrome";
import { McpCredentialCard } from "./mcp-credential";

/**
 * A trusted component an agent asked the client to draw (ADR 0083, `docs/api/trusted-components-v1.md`), from the
 * `vymalo.trustedRequest` activity: the client's own card, in its own chrome, and a receipt for the agent. Nothing
 * here comes from a surface, so nothing an agent draws can produce it.
 *
 * Only `McpCredential` is drawn (ADR 0046). Every other name is **words and a link**, never a plain form (decision 6):
 * the client does not guess at an interface it does not have.
 */
export function TrustedRequestCard({ data }: { data: unknown }) {
  const request = parseTrustedRequest(data);
  if (!request) return null;
  const drawn = isDrawn(request.component);
  // the frame's own title: what the card is when this build draws it, and what it is otherwise
  let title = "Something the agent asked this app to show";
  let body: ReactNode;
  if (!drawn) {
    body = (
      <TrustedFallback>
        <p>
          The agent asked this app to show <strong>{shownName(request.component)}</strong>, which
          this version does not have. Nothing was opened and nothing was sent.
        </p>
      </TrustedFallback>
    );
  } else if (!request.card) {
    // the card's id is what the receipt answers (`ThreadAgent` writes it); without one, nothing is drawn
    body = (
      <TrustedFallback>
        <p>This app cannot answer this request, because the card carries no id.</p>
      </TrustedFallback>
    );
  } else {
    title = "A key only you can see";
    body = <McpCredentialCard card={request.card} hints={request.hints} actor={request.actor} />;
  }
  return (
    <TrustedFrame actor={request.actor} title={title}>
      {body}
    </TrustedFrame>
  );
}
