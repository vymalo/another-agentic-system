import type { ErrorPartData } from "@/chat/to-items";
import { ActorLabel } from "../ActorLabel";

/** Inline error. Deliberately not `role="alert"`: a replay would announce every old error. */
export function ErrorLine({ data }: { data: ErrorPartData }) {
  return (
    <p className="error-line">
      <span className="error-line__icon" aria-hidden="true">
        !
      </span>
      <span className="error-line__body">
        <strong>Error:</strong> {data.message}
        <span className="error-line__hint">
          {data.retryable ? "You can send a message to retry." : "Start a new thread to try again."}
        </span>
      </span>
      <ActorLabel actor={data.actor} />
    </p>
  );
}
