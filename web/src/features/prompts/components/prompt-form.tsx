"use client";

import { PlusIcon } from "lucide-react";
import { type FormEvent, useEffect, useId, useRef, useState } from "react";
import { InlineStatus } from "@/components/inline-status";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { blankQuestion, type EditableQuestion, type FormQuestion } from "../lib/editor";
import type { Failure } from "../lib/failures";
import { byteLength, type PromptFields, slugName } from "../lib/library";
import { KeptQuestionCard } from "./kept-question";
import { controlSuffix, QuestionEditor } from "./question-editor";

export type FormOutcome = { ok: true } | { ok: false; failure: Failure };

type Props = {
  /** The heading: "New prompt", "Edit “Review”". */
  heading: string;
  /** What the person is making: the name follows the title only for a new one. */
  mode: "create" | "edit" | "copy";
  initial: PromptFields;
  limits: { maxVariables: number; maxBodyBytes: number };
  onSubmit: (fields: PromptFields) => Promise<FormOutcome>;
  onCancel: () => void;
  /** Edit only: reads the prompt again and starts the form from it. Answers why it could not, else null. */
  onLoadCurrent?: () => Promise<string | null>;
};

const joined = (...ids: (string | false | null | undefined)[]): string | undefined => {
  const out = ids.filter(Boolean).join(" ");
  return out === "" ? undefined : out;
};

const FIELD_ORDER = ["title", "name", "description", "body"] as const;

/**
 * The form of a prompt (create, edit, and copy of a shared one): title, `/name`, description, the text and its questions.
 * **The server judges every value**: this form bounds nothing beyond the size the configuration names, and shows what the
 * server answered where it says (`at`): the words under the field, `aria-invalid` and `aria-describedby` on it, the focus on it,
 * and for a reference the braces selected. A stale edit keeps everything typed and offers the current version; it is never sent
 * again with a newer revision, so it can never overwrite.
 */
export function PromptForm({
  heading,
  mode,
  initial,
  limits,
  onSubmit,
  onCancel,
  onLoadCurrent,
}: Props) {
  const id = useId();
  const headingRef = useRef<HTMLHeadingElement>(null);
  const bodyRef = useRef<HTMLTextAreaElement>(null);
  const caret = useRef<{ start: number; end: number } | null>(null);
  const [title, setTitle] = useState(initial.title);
  const [name, setName] = useState(initial.name);
  const [nameTouched, setNameTouched] = useState(mode !== "create" || initial.name !== "");
  const [description, setDescription] = useState(initial.description);
  const [body, setBody] = useState(initial.body);
  const [questions, setQuestions] = useState<FormQuestion[]>(initial.questions);
  const [pending, setPending] = useState(false);
  const [loading, setLoading] = useState(false);
  const [failure, setFailure] = useState<Failure | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [focusNew, setFocusNew] = useState<string | null>(null);
  const [moved, setMoved] = useState<{ key: string; dir: -1 | 1; text: string } | null>(null);

  // the form opens with the focus on its heading, which names what is being done
  useEffect(() => {
    headingRef.current?.focus();
  }, []);

  // a new question puts the focus on its own first control
  useEffect(() => {
    if (focusNew) document.getElementById(`${id}-${focusNew}-question`)?.focus();
  }, [focusNew, id]);

  // after a move the focus stays on the same control of the moved card, or on the other direction's when that one is now disabled
  // biome-ignore lint/correctness/useExhaustiveDependencies: `moved` is a new object for every move
  useEffect(() => {
    if (!moved) return;
    const base = `${id}-${moved.key}`;
    const same = document.getElementById(`${base}-${moved.dir < 0 ? "up" : "down"}`);
    const other = document.getElementById(`${base}-${moved.dir < 0 ? "down" : "up"}`);
    const pick = same instanceof HTMLButtonElement && !same.disabled ? same : other;
    if (pick instanceof HTMLElement) pick.focus();
  }, [moved]);

  // a refusal moves the focus to what it is about (a new refusal about the same field moves it again)
  // biome-ignore lint/correctness/useExhaustiveDependencies: `failure` is a new object for every refusal
  useEffect(() => {
    if (!failure) return;
    const target = failure.target;
    if (target.field === "title" || target.field === "name" || target.field === "description") {
      document.getElementById(`${id}-${target.field}`)?.focus();
    } else if (target.field === "body") {
      const el = bodyRef.current;
      if (!el) return;
      el.focus();
      if (failure.select) el.setSelectionRange(failure.select.start, failure.select.end);
    } else if (target.field === "question") {
      const q = questions[target.index];
      if (!q) return;
      const base = `${id}-${q.key}`;
      // the control of the member the problem names, else the card itself
      const suffix = q.kept ? null : controlSuffix(target.path);
      const control = suffix ? document.getElementById(`${base}-${suffix}`) : null;
      (control ?? document.getElementById(base))?.focus();
    } else if (target.field === "questions") {
      document.getElementById(`${id}-questions`)?.focus();
    }
  }, [failure]);

  const fields = (): PromptFields => ({ title, name, description, body, questions });
  const bytes = byteLength(body);
  const idsOf = (list: readonly FormQuestion[]) =>
    list.map((q) => (q.kept ? q.id : q.fields.id.trim()));

  async function submit(event: FormEvent) {
    event.preventDefault();
    if (pending || loading) return;
    setFailure(null);
    setPending(true);
    const outcome = await onSubmit(fields());
    setPending(false);
    if (!outcome.ok) setFailure(outcome.failure);
  }

  async function loadCurrent() {
    if (!onLoadCurrent) return;
    setLoading(true);
    setLoadError(null);
    const error = await onLoadCurrent();
    setLoading(false);
    if (error) setLoadError(error);
  }

  function insert(blank: string) {
    const text = `{{${blank}}}`;
    const at = caret.current ?? { start: body.length, end: body.length };
    const next = body.slice(0, at.start) + text + body.slice(at.end);
    const end = at.start + text.length;
    setBody(next);
    caret.current = { start: end, end };
    requestAnimationFrame(() => {
      const el = bodyRef.current;
      if (!el) return;
      el.focus();
      el.setSelectionRange(end, end);
    });
  }

  const setQuestion = (i: number, next: FormQuestion) =>
    setQuestions((list) => list.map((q, k) => (k === i ? next : q)));
  const moveQuestion = (i: number, delta: -1 | 1) => {
    const out = [...questions];
    const [item] = out.splice(i, 1);
    if (!item) return;
    out.splice(i + delta, 0, item);
    setQuestions(out);
    setMoved({
      key: item.key,
      dir: delta,
      text: `Question moved to position ${i + delta + 1} of ${out.length}`,
    });
  };
  const addQuestion = () => {
    const added = blankQuestion(questions);
    setQuestions((list) => [...list, added]);
    setFocusNew(added.key);
  };

  const field = (name: (typeof FIELD_ORDER)[number]) =>
    failure && failure.target.field === name ? failure.message : null;
  const errorIdOf = (name: string) => `${id}-${name}-error`;
  const formWords = failure && failure.target.field === "form" ? failure : null;
  // a refusal of "audience" is not a field of this form (a prompt is only ever private from here): it is the form's
  const topWords = formWords ?? (failure && failure.target.field === "audience" ? failure : null);
  const questionsError = failure && failure.target.field === "questions" ? failure.message : null;
  const atLimit = questions.length >= limits.maxVariables;
  const submitLabel = mode === "edit" ? "Save changes" : "Save prompt";

  return (
    <form
      onSubmit={submit}
      noValidate
      data-slot="prompt-form"
      data-mode={mode}
      aria-labelledby={`${id}-heading`}
      className="flex flex-col gap-5"
    >
      <h2
        id={`${id}-heading`}
        ref={headingRef}
        tabIndex={-1}
        className="text-lg leading-tight font-medium outline-none"
      >
        {heading}
      </h2>

      {topWords ? (
        <div className="flex flex-col gap-2">
          <InlineStatus tone={topWords.stale ? "warning" : "error"} role="alert">
            {topWords.message}
          </InlineStatus>
          {topWords.stale && onLoadCurrent ? (
            <Button
              type="button"
              variant="outline"
              size="sm"
              className="self-start"
              disabled={loading}
              onClick={loadCurrent}
            >
              {loading ? "Loading…" : "Load the current version"}
            </Button>
          ) : null}
          {loadError ? (
            <InlineStatus tone="error" role="alert">
              {loadError}
            </InlineStatus>
          ) : null}
        </div>
      ) : null}

      <div className="flex flex-col gap-1.5">
        <Label htmlFor={`${id}-title`}>Title</Label>
        <Input
          id={`${id}-title`}
          value={title}
          autoComplete="off"
          aria-invalid={field("title") ? true : undefined}
          aria-describedby={joined(field("title") && errorIdOf("title"))}
          onChange={(e) => {
            setTitle(e.target.value);
            if (!nameTouched) setName(slugName(e.target.value));
          }}
        />
        {field("title") ? (
          <p id={errorIdOf("title")} role="alert" className="text-sm text-destructive">
            {field("title")}
          </p>
        ) : null}
      </div>

      <div className="flex flex-col gap-1.5">
        <Label htmlFor={`${id}-name`}>Name</Label>
        <div className="flex items-center gap-2">
          <span aria-hidden="true" className="font-mono text-sm text-muted-foreground">
            /
          </span>
          <Input
            id={`${id}-name`}
            value={name}
            autoComplete="off"
            autoCapitalize="none"
            spellCheck={false}
            className="font-mono"
            aria-invalid={field("name") ? true : undefined}
            aria-describedby={joined(`${id}-name-help`, field("name") && errorIdOf("name"))}
            onChange={(e) => {
              setName(e.target.value);
              setNameTouched(true);
            }}
          />
        </div>
        <p id={`${id}-name-help`} className="text-xs text-muted-foreground">
          Type /name in a message box to use it. Lowercase letters, digits and hyphens.
        </p>
        {field("name") ? (
          <p id={errorIdOf("name")} role="alert" className="text-sm text-destructive">
            {field("name")}
          </p>
        ) : null}
      </div>

      <div className="flex flex-col gap-1.5">
        <Label htmlFor={`${id}-description`}>Description (optional)</Label>
        <Input
          id={`${id}-description`}
          value={description}
          autoComplete="off"
          aria-invalid={field("description") ? true : undefined}
          aria-describedby={joined(field("description") && errorIdOf("description"))}
          onChange={(e) => setDescription(e.target.value)}
        />
        {field("description") ? (
          <p id={errorIdOf("description")} role="alert" className="text-sm text-destructive">
            {field("description")}
          </p>
        ) : null}
      </div>

      <div className="flex flex-col gap-1.5">
        <Label htmlFor={`${id}-body`}>Text</Label>
        <textarea
          id={`${id}-body`}
          ref={bodyRef}
          value={body}
          rows={8}
          spellCheck
          data-slot="textarea"
          aria-invalid={field("body") ? true : undefined}
          aria-describedby={joined(
            `${id}-body-help`,
            `${id}-body-count`,
            field("body") && errorIdOf("body"),
          )}
          className="min-h-32 w-full rounded-md border border-input bg-transparent px-2.5 py-2 font-mono text-base shadow-xs transition-[color,box-shadow] outline-none selection:bg-primary selection:text-primary-foreground placeholder:text-muted-foreground focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50 aria-invalid:border-destructive aria-invalid:ring-3 aria-invalid:ring-destructive/20 md:text-sm dark:bg-input/30 dark:aria-invalid:border-destructive/50 dark:aria-invalid:ring-destructive/40"
          onChange={(e) => {
            setBody(e.target.value);
            caret.current = { start: e.target.selectionStart, end: e.target.selectionEnd };
          }}
          onSelect={(e) => {
            const el = e.currentTarget;
            caret.current = { start: el.selectionStart, end: el.selectionEnd };
          }}
        />
        <p id={`${id}-body-help`} className="text-xs text-muted-foreground">
          Plain text. Write {"{{name}}"} where an answer goes, for example {"{{focus}}"}. Type{" "}
          {"\\{{"} for two literal braces.
        </p>
        <p
          id={`${id}-body-count`}
          className={`text-xs tabular-nums ${bytes > limits.maxBodyBytes ? "text-destructive" : "text-muted-foreground"}`}
        >
          {bytes.toLocaleString("en-US")} of {limits.maxBodyBytes.toLocaleString("en-US")} bytes
        </p>
        {field("body") ? (
          <p id={errorIdOf("body")} role="alert" className="text-sm text-destructive">
            {field("body")}
          </p>
        ) : null}
      </div>

      <section aria-labelledby={`${id}-questions`} className="flex flex-col gap-3">
        <h3
          id={`${id}-questions`}
          tabIndex={-1}
          aria-describedby={questionsError ? errorIdOf("questions") : undefined}
          className="text-sm font-medium outline-none"
        >
          Questions{" "}
          <span className="font-normal text-muted-foreground">
            ({questions.length} of {limits.maxVariables})
          </span>
        </h3>
        <p className="text-sm text-muted-foreground">
          A question is asked when the prompt is used, and its answer goes where its name is written
          in the text.
        </p>
        {questionsError ? (
          <p id={errorIdOf("questions")} role="alert" className="text-sm text-destructive">
            {questionsError}
          </p>
        ) : null}
        <p role="status" aria-live="polite" className="sr-only">
          {moved?.text ?? ""}
        </p>
        {questions.length > 0 ? (
          <ol aria-label="Questions" className="flex flex-col gap-3">
            {questions.map((q, i) => {
              const base = `${id}-${q.key}`;
              const refusal =
                failure && failure.target.field === "question" && failure.target.index === i
                  ? {
                      message: failure.message,
                      suffix: q.kept ? null : controlSuffix(failure.target.path),
                    }
                  : null;
              return q.kept ? (
                <KeptQuestionCard
                  key={q.key}
                  q={q}
                  index={i}
                  count={questions.length}
                  baseId={base}
                  busy={pending}
                  refusal={refusal}
                  onMove={(d) => moveQuestion(i, d)}
                  onRemove={() => setQuestions((list) => list.filter((_, k) => k !== i))}
                />
              ) : (
                <QuestionEditor
                  key={q.key}
                  q={q}
                  index={i}
                  count={questions.length}
                  baseId={base}
                  busy={pending}
                  otherIds={idsOf(questions.filter((_, k) => k !== i))}
                  refusal={refusal}
                  onChange={(next: EditableQuestion) => setQuestion(i, next)}
                  onMove={(d) => moveQuestion(i, d)}
                  onRemove={() => setQuestions((list) => list.filter((_, k) => k !== i))}
                  onInsert={insert}
                />
              );
            })}
          </ol>
        ) : null}
        {atLimit ? (
          <p className="text-sm text-muted-foreground">
            A prompt has at most {limits.maxVariables} questions.
          </p>
        ) : (
          <Button type="button" variant="outline" className="self-start" onClick={addQuestion}>
            <PlusIcon aria-hidden="true" />
            Add a question
          </Button>
        )}
      </section>

      <div className="flex flex-wrap items-center gap-2">
        <Button type="submit" disabled={pending || loading}>
          {pending ? "Saving…" : submitLabel}
        </Button>
        <Button type="button" variant="ghost" onClick={onCancel} disabled={pending}>
          Cancel
        </Button>
      </div>
    </form>
  );
}
