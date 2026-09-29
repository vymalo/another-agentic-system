import { CircleAlertIcon } from "lucide-react";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import type { ErrorContent } from "@/features/chat/lib/agui/vymalo";
import { ActorLabel } from "../actor-label";

/**
 * Inline error. Deliberately not `role="alert"`: a replay would announce every old error, so the
 * role is replaced by `none` (the `Alert` sets `alert` by default).
 */
export function ErrorLine({ data }: { data: ErrorContent }) {
  return (
    <Alert variant="destructive" role="none" className="max-w-xl">
      <CircleAlertIcon aria-hidden="true" />
      <AlertTitle className="[overflow-wrap:anywhere]">
        <strong>Error:</strong> {data.message}
      </AlertTitle>
      <AlertDescription className="flex flex-wrap items-baseline justify-between gap-x-3">
        <span>
          {data.retryable ? "You can send a message to retry." : "Start a new thread to try again."}
        </span>
        <ActorLabel actor={data.actor} />
      </AlertDescription>
    </Alert>
  );
}
