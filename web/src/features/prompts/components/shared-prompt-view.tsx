"use client";

import { ArrowLeftIcon } from "lucide-react";
import { useEffect, useRef } from "react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import type { ApiPrompt } from "@/lib/api/types";

type Props = {
  prompt: ApiPrompt;
  /** Whether Copy is offered (not at the limit). */
  canCopy: boolean;
  onBack: () => void;
  onCopy: () => void;
};

const KIND_WORDS: Record<string, string> = {
  single: "One choice",
  multiple: "Several choices",
  select: "One or several from a long list",
  text: "Short text",
  longText: "Long text",
  confirm: "Yes or no",
  number: "Number",
  scale: "Scale",
  date: "Date",
  rank: "Ranking",
};

/**
 * A prompt somebody shared with the person, read only: its text, its questions and nothing else. It names no author (the API gives
 * none) and is drawn as plain text, never as Markdown or HTML. The one thing to do with it is to copy it into one's own prompts,
 * which is a client's recipe: read the shared prompt, create an own one from what was read.
 */
export function SharedPromptView({ prompt, canCopy, onBack, onCopy }: Props) {
  const heading = useRef<HTMLHeadingElement>(null);
  useEffect(() => {
    heading.current?.focus();
  }, []);
  return (
    <article
      data-slot="shared-prompt"
      aria-labelledby="shared-prompt-title"
      className="flex flex-col gap-5"
    >
      <div className="flex flex-col gap-2">
        <div className="flex flex-wrap items-baseline gap-x-3 gap-y-1">
          <h2
            id="shared-prompt-title"
            ref={heading}
            tabIndex={-1}
            className="text-lg leading-tight font-medium break-words outline-none"
          >
            {prompt.title}
          </h2>
          <Badge variant="secondary">Shared with you</Badge>
        </div>
        <p className="font-mono text-sm break-all text-muted-foreground">/{prompt.name}</p>
        {prompt.description ? (
          <p className="text-sm break-words whitespace-pre-line text-muted-foreground">
            {prompt.description}
          </p>
        ) : null}
      </div>
      <section aria-labelledby="shared-prompt-text" className="flex flex-col gap-1.5">
        <h3 id="shared-prompt-text" className="text-sm font-medium">
          Text
        </h3>
        <pre
          data-slot="shared-prompt-body"
          className="max-h-96 overflow-auto rounded-md border bg-muted/40 p-3 font-mono text-sm break-words whitespace-pre-wrap"
        >
          {prompt.body}
        </pre>
      </section>
      <section aria-labelledby="shared-prompt-questions" className="flex flex-col gap-2">
        <h3 id="shared-prompt-questions" className="text-sm font-medium">
          Questions{" "}
          <span className="font-normal text-muted-foreground">({prompt.variables.length})</span>
        </h3>
        {prompt.variables.length === 0 ? (
          <p className="text-sm text-muted-foreground">It asks no questions.</p>
        ) : (
          <ol className="flex flex-col gap-2">
            {prompt.variables.map((q) => (
              <li key={q.id} className="rounded-lg border bg-card px-3 py-2 text-sm">
                <p className="font-medium break-words">{q.question}</p>
                <p className="text-xs text-muted-foreground">
                  {KIND_WORDS[q.type ?? "single"] ?? "Another kind of question"} ·{" "}
                  {q.required === false ? "optional" : "required"}
                  {q.options?.length ? (
                    <span className="break-words">
                      {" "}
                      · {q.options.map((o) => o.label).join(", ")}
                    </span>
                  ) : null}
                </p>
              </li>
            ))}
          </ol>
        )}
      </section>
      <div className="flex flex-wrap items-center gap-2">
        {canCopy ? (
          <Button type="button" onClick={onCopy}>
            Copy to my prompts
          </Button>
        ) : (
          <p className="text-sm text-muted-foreground">
            You have the most prompts one person can have. Delete one to copy this one.
          </p>
        )}
        <Button type="button" variant="ghost" onClick={onBack}>
          <ArrowLeftIcon aria-hidden="true" />
          Back to your prompts
        </Button>
      </div>
    </article>
  );
}
