import { MessagePrimitive, useAuiState } from "@assistant-ui/react";
import type { ApiActor } from "@/api/types";
import type { ArtifactPartData, ErrorPartData, StatusPartData } from "@/chat/to-items";
import { ActorLabel } from "./ActorLabel";
import { MarkdownText } from "./MarkdownText";
import { ArtifactCard } from "./parts/ArtifactCard";
import { ErrorLine } from "./parts/ErrorLine";
import { StatusLine } from "./parts/StatusLine";

type DataProps<T> = { data: T };
// assistant-ui passes `{ name, data }` to data-part renderers; adapt to our typed props.
const Status = ({ data }: DataProps<StatusPartData>) => <StatusLine data={data} />;
const Artifact = ({ data }: DataProps<ArtifactPartData>) => <ArtifactCard data={data} />;
const Error_ = ({ data }: DataProps<ErrorPartData>) => <ErrorLine data={data} />;

const DATA = { by_name: { status: Status, artifact: Artifact, error: Error_ } };

const useActor = (): ApiActor | undefined =>
  useAuiState((s) => (s.message.metadata.custom as { actor?: ApiActor } | undefined)?.actor);
const useIsText = (): boolean => useAuiState((s) => s.message.content[0]?.type === "text");

export function UserMessage() {
  const actor = useActor();
  return (
    <MessagePrimitive.Root className="msg msg--user">
      <div className="bubble bubble--user">
        <MessagePrimitive.Parts components={{ Text: MarkdownText }} />
      </div>
      <div className="msg__actor">
        <ActorLabel actor={actor} />
      </div>
    </MessagePrimitive.Root>
  );
}

export function AssistantMessage() {
  const actor = useActor();
  const isText = useIsText();
  return (
    <MessagePrimitive.Root className={`msg msg--agent ${isText ? "" : "msg--inline"}`}>
      {isText ? (
        <div className="msg__actor">
          <ActorLabel actor={actor} />
        </div>
      ) : null}
      <div className={isText ? "bubble bubble--agent" : "inline-part"}>
        <MessagePrimitive.Parts components={{ Text: MarkdownText, data: DATA }} />
      </div>
    </MessagePrimitive.Root>
  );
}
