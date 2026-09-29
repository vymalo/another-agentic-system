import { ThreadPrimitive } from "@assistant-ui/react";
import { InlineStatus } from "./InlineStatus";
import { AssistantMessage, UserMessage } from "./Messages";

export function Thread({ empty }: { empty: boolean }) {
  return (
    <ThreadPrimitive.Root className="thread">
      <ThreadPrimitive.Viewport className="thread__viewport">
        <div className="thread__log" aria-label="Conversation" role="log">
          {empty ? <InlineStatus>Waiting for the first event…</InlineStatus> : null}
          <ThreadPrimitive.Messages>
            {({ message }) => (message.role === "user" ? <UserMessage /> : <AssistantMessage />)}
          </ThreadPrimitive.Messages>
        </div>
      </ThreadPrimitive.Viewport>
    </ThreadPrimitive.Root>
  );
}
