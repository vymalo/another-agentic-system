"use client";

import { useAuiState } from "@assistant-ui/react";
import { createContext, type ReactNode, useContext, useMemo } from "react";
import { parseDelivery } from "@/features/chat/lib/agui/vymalo";
import { deliveryNote } from "@/features/chat/lib/send";

/**
 * What the note under a message needs (ADR 0036): the words around how a message was delivered,
 * which are the agent's name and what its card lists now. How it was delivered is the log's, in the
 * message's metadata.
 */
type Delivery = {
  /** Who was working ("Coder"). */
  agent: string;
  /** Whether the agent's card lists `steer/v1`; null when it could not be read (`readsWhen`). */
  steers: boolean | null;
};

const Context = createContext<Delivery>({ agent: "the agent", steers: null });

export function DeliveryProvider({ agent, steers, children }: Delivery & { children: ReactNode }) {
  const value = useMemo(() => ({ agent, steers }), [agent, steers]);
  return <Context.Provider value={value}>{children}</Context.Provider>;
}

/**
 * The quiet line under a message the person sent while the agent worked: "Sent while Coder was
 * working · read at its next step", or, for Stop and send, "Stopped Coder · it starts again from
 * here". The message comes back from the log (`ThreadAgent.sendWhileWorking`), and its delivery is
 * in its metadata (`live-runs.ts`). A message sent to an agent that was idle has none.
 */
export function DeliveryNote() {
  const { agent, steers } = useContext(Context);
  const mode = parseDelivery(useAuiState((s) => s.message.metadata?.custom?.delivery));
  if (!mode) return null;
  return (
    <p
      data-slot="delivery-note"
      data-delivery={mode}
      className="mt-1 max-w-[85%] text-end text-xs leading-4 text-muted-foreground sm:max-w-[80%]"
    >
      {deliveryNote(mode, agent, steers)}
    </p>
  );
}
