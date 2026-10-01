"use client";

import { type ReactNode, useId, useState } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  type AnswerState,
  answerContext,
  type ChoiceOption,
  type ChoiceQuestion,
  EMPTY,
  MAX_OTHER,
  missing,
  type QuestionState,
  readChoices,
} from "@/features/chat/lib/a2ui/choices";
import { contextTooLarge } from "@/features/chat/lib/a2ui/context";
import { MAX_CONTEXT_BYTES } from "@/features/chat/lib/a2ui/limits";
import { useSurfaceHost } from "./surface-host";
import { useView } from "./view-context";

/*
 * The `Choices` component of the UI catalog (docs/api/ui-catalog-v1.md, version 2): up to 8
 * questions asked at once, each a group of radio buttons (checkboxes when `multiple`), optionally
 * with an "Other" of the person's own words. One submit sends ONE action, and nothing sends but
 * that click (ADR 0013 rule 7): Enter in the "Other" box does nothing, because there is no form.
 *
 * What reaches this file has passed the schema of the catalog and `prepareSurface`; the strings
 * are the agent's, drawn as React text. The controls are native inputs in a `fieldset`: the
 * browser gives the arrow keys, the roving tab stop and the screen reader's group for free, and
 * each question's group is named by its legend.
 */

const TILE =
  "relative flex min-h-11 gap-3 rounded-lg border bg-background px-3 py-2.5 text-sm transition-colors " +
  "has-[:checked]:border-primary has-[:checked]:bg-primary/5 " +
  "has-[:focus-visible]:ring-3 has-[:focus-visible]:ring-ring/50 " +
  "has-[:disabled]:cursor-not-allowed has-[:disabled]:opacity-60";
const CONTROL = "mt-0.5 size-4 shrink-0 accent-primary disabled:cursor-not-allowed";

/** One option: the whole tile is the click target (the label's `::after` covers it). */
function OptionTile({
  option,
  type,
  name,
  checked,
  onChange,
}: {
  option: ChoiceOption;
  type: "radio" | "checkbox";
  name: string;
  checked: boolean;
  onChange: () => void;
}) {
  const id = useId();
  const described = `${id}-d`;
  return (
    <div data-slot="choices-option" className={TILE}>
      <input
        id={id}
        type={type}
        name={name}
        value={option.value}
        checked={checked}
        onChange={onChange}
        aria-describedby={option.description ? described : undefined}
        className={CONTROL}
      />
      <span className="flex min-w-0 flex-col">
        <label
          htmlFor={id}
          className="cursor-pointer [overflow-wrap:anywhere] after:absolute after:inset-0 after:content-['']"
        >
          {option.label}
        </label>
        {option.description ? (
          <span id={described} className="text-xs text-muted-foreground [overflow-wrap:anywhere]">
            {option.description}
          </span>
        ) : null}
      </span>
    </div>
  );
}

/** "Other": a choice of its own and the person's words; typing in the box turns it on. */
function OtherTile({
  question,
  type,
  name,
  state,
  onChange,
}: {
  question: ChoiceQuestion;
  type: "radio" | "checkbox";
  name: string;
  state: QuestionState;
  onChange: (patch: Partial<QuestionState>) => void;
}) {
  const id = useId();
  const single = type === "radio";
  return (
    <div data-slot="choices-other" className={`${TILE} flex-wrap items-center`}>
      <input
        id={id}
        type={type}
        name={name}
        value="other"
        checked={state.otherOn}
        onChange={(e) =>
          onChange(single ? { otherOn: true, values: [] } : { otherOn: e.target.checked })
        }
        className={`${CONTROL} mt-0`}
      />
      <label htmlFor={id} className="cursor-pointer">
        Other
      </label>
      <Input
        aria-label={`Your own answer to: ${question.question}`}
        value={state.other}
        maxLength={MAX_OTHER}
        onChange={(e) =>
          onChange(
            single
              ? { other: e.target.value, otherOn: true, values: [] }
              : { other: e.target.value, otherOn: true },
          )
        }
        className="min-w-40 flex-1"
      />
    </div>
  );
}

function Question({
  question,
  name,
  state,
  enabled,
  onChange,
}: {
  question: ChoiceQuestion;
  name: string;
  state: QuestionState;
  enabled: boolean;
  onChange: (patch: Partial<QuestionState>) => void;
}) {
  const legend = useId();
  const type = question.multiple ? "checkbox" : "radio";
  return (
    <fieldset
      data-slot="choices-question"
      // a radiogroup for the radio buttons, a plain group for the checkboxes: both named by the legend
      role={question.multiple ? undefined : "radiogroup"}
      aria-labelledby={legend}
      disabled={!enabled}
      className="m-0 flex min-w-0 flex-col gap-2 border-0 p-0"
    >
      <legend id={legend} className="mb-1 p-0 text-sm font-medium [overflow-wrap:anywhere]">
        {question.question}
        {question.required ? null : (
          <span className="font-normal text-muted-foreground"> (optional)</span>
        )}
      </legend>
      {question.options.map((option) => (
        <OptionTile
          key={option.value}
          option={option}
          type={type}
          name={name}
          checked={state.values.includes(option.value)}
          onChange={() =>
            onChange(
              question.multiple
                ? {
                    values: state.values.includes(option.value)
                      ? state.values.filter((v) => v !== option.value)
                      : [...state.values, option.value],
                  }
                : { values: [option.value], otherOn: false },
            )
          }
        />
      ))}
      {question.allowOther ? (
        <OtherTile question={question} type={type} name={name} state={state} onChange={onChange} />
      ) : null}
    </fieldset>
  );
}

/** The Choices of a surface; the props are those of the lowered component (`prepare.ts`). */
export function ChoicesInput(props: Record<string, unknown>): ReactNode {
  const view = useView();
  const host = useSurfaceHost();
  const base = useId();
  const [state, setState] = useState<AnswerState>(new Map());
  const spec = readChoices(props);
  if (!spec) return null;

  // the answers go to the agent while the thread waits for them, from the newest copy only; once
  // sent (or in a thread that moved on) the choices stay as they were, and stop taking input
  const enabled = view.live && host.canSend;
  const left = missing(spec, state);
  const patch = (id: string, p: Partial<QuestionState>) =>
    setState((s) => new Map(s).set(id, { ...(s.get(id) ?? EMPTY), ...p }));

  const send = () => {
    if (!enabled || left > 0) return;
    const context = answerContext(spec, state);
    if (contextTooLarge(context)) {
      host.reject(`The answers carry more than ${MAX_CONTEXT_BYTES / 1024} KiB.`);
      return;
    }
    host.send({
      name: spec.actionName,
      surfaceId: view.surfaceId,
      sourceComponentId: spec.componentId,
      context,
    });
  };

  const hint = `${base}-hint`;
  return (
    <div data-slot="choices" className="flex min-w-0 flex-col gap-4">
      {spec.title ? (
        // biome-ignore lint/a11y/useSemanticElements: the level is the surface's, not the page's
        <p
          role="heading"
          aria-level={3}
          className="text-base font-semibold [overflow-wrap:anywhere]"
        >
          {spec.title}
        </p>
      ) : null}
      {spec.questions.map((q) => (
        <Question
          key={q.id}
          question={q}
          name={`${base}-${q.id}`}
          state={state.get(q.id) ?? EMPTY}
          enabled={enabled}
          onChange={(p) => patch(q.id, p)}
        />
      ))}
      <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
        <Button
          type="button"
          disabled={!enabled || left > 0}
          aria-describedby={enabled && left > 0 ? hint : undefined}
          onClick={send}
        >
          {spec.submitLabel ?? "Send answers"}
        </Button>
        {enabled && left > 0 ? (
          <p id={hint} className="text-xs text-muted-foreground">
            {left === 1
              ? "1 question is not answered yet."
              : `${left} questions are not answered yet.`}
          </p>
        ) : null}
      </div>
    </div>
  );
}
