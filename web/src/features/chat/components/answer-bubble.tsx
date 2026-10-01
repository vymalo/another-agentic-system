"use client";

import { useAuiState } from "@assistant-ui/react";
import { useId, useMemo } from "react";
import { answerLines, questionsOf } from "@/features/chat/lib/a2ui/choices";
import { surfaceOperations } from "@/features/chat/lib/a2ui/surfaces";
import type { AnswersContent } from "@/features/chat/lib/agui/vymalo";

/**
 * The person's answer to a `Choices` (docs/api/ui-catalog-v1.md, "Choices answers"): what an
 * action whose `context.answers` has that shape reads as. It is the person's, so it is drawn as
 * theirs, a soft bubble on the right under "Your answers" (web/DESIGN.md, "A turn"): each
 * question with the labels they chose, resolved from the surface the action names, in the
 * transcript. A question or a value that surface does not have shows as it was sent (its id, the
 * raw value), never as nothing. Every string here is the agent's or the person's: text only.
 */
export function AnswerBubble({ data }: { data: AnswersContent }) {
  // the operations array itself, so the selector returns the same reference until the transcript changes it
  const operations = useAuiState((s) =>
    surfaceOperations(s.thread.messages, s.message.id, data.surfaceId),
  );
  const heading = useId();
  const lines = useMemo(
    () =>
      answerLines(data.answers, questionsOf(operations, data.surfaceId, data.sourceComponentId)),
    [operations, data.answers, data.surfaceId, data.sourceComponentId],
  );
  return (
    <div
      data-slot="answer-bubble"
      className="max-w-[85%] min-w-0 self-end rounded-[20px] rounded-tr-md bg-bubble px-4 py-3 text-[0.9375rem] leading-6 sm:max-w-[80%]"
    >
      <p id={heading} className="mb-1.5 text-xs font-medium text-muted-foreground">
        Your answers
      </p>
      <dl aria-labelledby={heading} className="m-0 flex flex-col gap-2">
        {lines.map((line) => (
          <div key={line.id} data-slot="answer-line" className="min-w-0">
            <dt className="text-[0.8125rem] leading-5 text-muted-foreground [overflow-wrap:anywhere]">
              {line.question}
            </dt>
            <dd className="m-0 [overflow-wrap:anywhere]">
              {line.chosen.length === 0 && line.other === undefined ? (
                <span className="text-muted-foreground">No answer</span>
              ) : (
                [
                  ...line.chosen,
                  ...(line.other !== undefined ? [`Other: ${line.other}`] : []),
                ].join(", ")
              )}
            </dd>
          </div>
        ))}
      </dl>
      {data.at ? (
        <time dateTime={data.at} className="sr-only">
          {new Date(data.at).toLocaleString()}
        </time>
      ) : null}
    </div>
  );
}
