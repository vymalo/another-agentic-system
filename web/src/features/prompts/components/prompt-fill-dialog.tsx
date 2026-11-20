"use client";

import { useEffect, useId, useRef, useState } from "react";
import { InlineStatus } from "@/components/inline-status";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Question } from "@/features/chat/components/surface/choices";
import {
  type Answer,
  type AnswerState,
  buildAnswers,
  type ChoicesSpec,
  EMPTY,
  initialState,
  missing,
  type QuestionState,
} from "@/features/chat/lib/a2ui/choices";
import type { ApiPrompt } from "@/lib/api/types";
import type { RenderResult } from "../hooks/use-prompt-picker";
import { type FillFailure, specOf } from "../lib/fill";
import type { PromptRead } from "../lib/read";

type Props = {
  prompt: ApiPrompt;
  /** `renderPrompt` for the answers: the text, or the words of the refusal. */
  fill: (revision: string, answers: Answer[], spec: ChoicesSpec) => Promise<RenderResult>;
  /** `getPrompt` again, for "Load the current version". */
  loadCurrent: () => Promise<PromptRead>;
  onFilled: (text: string) => void;
  onClose: () => void;
};

/**
 * The form that fills a prompt's variables (docs/api/prompts-v1.md, "The recipe"): its questions are
 * the catalog's own `Choices`, drawn by the catalog renderer the conversation uses, and one click
 * sends them to `renderPrompt`. The answer's text goes into the message box for the person to edit;
 * nothing here sends a message. A stale prompt is never overwritten: the form says so and offers
 * **Load the current version**, which keeps the answers whose questions still exist.
 */
export function PromptFillDialog({ prompt, fill, loadCurrent, onFilled, onClose }: Props) {
  const [current, setCurrent] = useState(prompt);
  const [spec, setSpec] = useState<ChoicesSpec | undefined>(() => specOf(prompt));
  const [answers, setAnswers] = useState<AnswerState>(() => {
    const first = specOf(prompt);
    return first ? initialState(first) : new Map();
  });
  const [failure, setFailure] = useState<FillFailure | null>(null);
  const [busy, setBusy] = useState(false);
  const heading = useRef<HTMLHeadingElement>(null);
  const content = useRef<HTMLDivElement>(null);
  const alive = useRef(true);
  const formId = useId();
  const hintId = `${formId}-hint`;

  useEffect(() => {
    // StrictMode runs the cleanup and the effect again: the ref is set true in the body, not only at mount
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);

  const left = spec ? missing(spec, answers) : 0;

  /** The answers that still have a question after a fresh read, over the defaults of the new ones. */
  const merge = (next: ChoicesSpec | undefined, before: AnswerState): AnswerState => {
    if (!next) return new Map();
    const base = initialState(next);
    const merged = new Map<string, QuestionState>();
    for (const q of next.questions) {
      merged.set(q.id, before.get(q.id) ?? base.get(q.id) ?? EMPTY);
    }
    return merged;
  };

  const applyCurrent = (next: ApiPrompt) => {
    const nextSpec = specOf(next);
    setCurrent(next);
    setSpec(nextSpec);
    setAnswers((before) => merge(nextSpec, before));
    setFailure(null);
  };

  const load = async () => {
    setBusy(true);
    const result = await loadCurrent();
    if (!alive.current) return;
    setBusy(false);
    if (result.ok) applyCurrent(result.value);
    else {
      setFailure({
        message: result.failure.message,
        ...(result.failure.gone ? { gone: true } : {}),
      });
    }
  };

  const submit = async () => {
    if (!spec || busy || left > 0 || failure?.gone) return;
    setBusy(true);
    setFailure(null);
    const result = await fill(current.revision, buildAnswers(spec, answers), spec);
    if (!alive.current) return;
    setBusy(false);
    if (result.ok) {
      onFilled(result.text);
      return;
    }
    setFailure(result.failure);
    focusQuestion(result.failure.question);
  };

  const focusQuestion = (id: string | undefined) => {
    if (!id) return;
    requestAnimationFrame(() => {
      const wrapper = [
        ...(content.current?.querySelectorAll<HTMLElement>("[data-prompt-question]") ?? []),
      ].find((el) => el.dataset.promptQuestion === id);
      wrapper?.querySelector<HTMLElement>("input, textarea, select, button")?.focus();
    });
  };

  const patch = (id: string, change: Partial<QuestionState>) =>
    setAnswers((state) => new Map(state).set(id, { ...(state.get(id) ?? EMPTY), ...change }));

  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open) onClose();
      }}
    >
      <DialogContent
        ref={content}
        data-slot="prompt-fill"
        onOpenAutoFocus={(event) => {
          event.preventDefault();
          heading.current?.focus();
        }}
      >
        <DialogHeader>
          <DialogTitle ref={heading} tabIndex={-1} className="outline-none">
            {current.title}
          </DialogTitle>
          <DialogDescription className="whitespace-pre-line">
            <span className="font-mono">/{current.name}</span>
            {current.description ? ` — ${current.description}` : null}
          </DialogDescription>
        </DialogHeader>

        {!spec ? (
          <InlineStatus tone="error" role="alert">
            This prompt asks a kind of question this client cannot draw. Nothing was filled.
          </InlineStatus>
        ) : (
          <>
            {spec.questions.length === 0 ? (
              <p className="text-sm text-muted-foreground">
                This prompt asks no questions. Use it to put its text in the message box.
              </p>
            ) : (
              <div className="flex flex-col gap-4">
                {spec.questions.map((question) => (
                  <div key={question.id} data-prompt-question={question.id}>
                    <Question
                      question={question}
                      name={`${formId}-${question.id}`}
                      state={answers.get(question.id) ?? EMPTY}
                      enabled={!busy && !failure?.gone}
                      onChange={(change) => patch(question.id, change)}
                    />
                  </div>
                ))}
              </div>
            )}
            {failure ? (
              <InlineStatus
                tone="error"
                role="alert"
                {...(failure.stale
                  ? {
                      action: {
                        label: "Load the current version",
                        onClick: () => void load(),
                      },
                    }
                  : {})}
              >
                {failure.message}
              </InlineStatus>
            ) : null}
          </>
        )}

        {spec && left > 0 ? (
          <p id={hintId} className="text-xs text-muted-foreground">
            {left === 1
              ? "1 question is not answered yet."
              : `${left} questions are not answered yet.`}
          </p>
        ) : null}

        <DialogFooter>
          <Button type="button" variant="ghost" onClick={onClose}>
            {spec ? "Cancel" : "Close"}
          </Button>
          {spec ? (
            <Button
              type="button"
              onClick={() => void submit()}
              aria-disabled={busy || left > 0 || failure?.gone === true || undefined}
              aria-busy={busy || undefined}
              aria-describedby={left > 0 ? hintId : undefined}
              className="aria-disabled:opacity-50"
            >
              Use prompt
            </Button>
          ) : null}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
