"use client";

import { ArrowDownIcon, ArrowUpIcon, CircleAlertIcon } from "lucide-react";
import { type ReactNode, useId, useLayoutEffect, useRef, useState } from "react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { STEP_ICON } from "@/features/chat/components/steps/step-icons";
import {
  type ChoiceQuestion,
  type QuestionState,
  valuesOf,
} from "@/features/chat/lib/a2ui/choices";
import {
  checkValues,
  NO,
  noLabel,
  scalePoints,
  YES,
  yesLabel,
} from "@/features/chat/lib/a2ui/choices-kinds";
import type { StepIcon } from "@/features/chat/lib/agui/vymalo";
import { cn } from "@/lib/utils";
import { Controls, groupDisabled, groupOf, Shown } from "./choices-shown";

/*
 * The five kinds of question of catalog version 7 (docs/api/ui-catalog-v1.md, "Version 7"): a yes or
 * no, a number, a point on a scale, a day and a list to put in order. What they send is `values` of
 * strings, and nothing sends but the form's one button: the controls only change what the person
 * has answered. Each one validates with the same check the contract gives a receiver
 * (`checkValues`) and says why in words next to the control, never by colour alone; the form holds
 * its button until every question is valid.
 *
 * The yes or no and the scale are native radio buttons in a `radiogroup` (the browser gives the arrow
 * keys and one tab stop); a number is a text box with `inputmode="decimal"` (a browser number box
 * reads locales and exponents, which are not the grammar); a day is the browser's own date control;
 * a rank is an ordered list with Move up and Move down buttons, operable with the keyboard alone.
 */

type Patch = (patch: Partial<QuestionState>) => void;
type Props = {
  question: ChoiceQuestion;
  /** The name the radio buttons of this question share. */
  name: string;
  state: QuestionState;
  enabled: boolean;
  onChange: Patch;
  /** What the question shows (catalog version 8), drawn between its words and its control. */
  shown?: readonly ReactNode[] | undefined;
};

const TILE =
  "relative flex min-h-11 items-center gap-2 rounded-lg border bg-background px-3 py-2 text-sm transition-colors " +
  "has-[:checked]:border-brand has-[:checked]:bg-brand/5 " +
  "has-[:focus-visible]:ring-3 has-[:focus-visible]:ring-ring/50 " +
  "has-[:disabled]:cursor-not-allowed has-[:disabled]:opacity-60";
const RADIO = "size-4 shrink-0 accent-brand disabled:cursor-not-allowed";

/** The question's `(optional)` mark: in words, as the other questions have it. */
const optional = (q: ChoiceQuestion) =>
  q.required ? null : <span className="font-normal text-muted-foreground"> (optional)</span>;

/** A line of words under a control that says what is wrong. Always in the page, so a change is announced. */
function Problem({ id, text }: { id: string; text: string | undefined }) {
  return (
    <p
      id={id}
      aria-live="polite"
      data-slot="choices-problem"
      className={text ? "flex items-start gap-1.5 text-xs text-destructive" : "sr-only"}
    >
      {text ? <CircleAlertIcon aria-hidden="true" className="mt-0.5 size-3.5 shrink-0" /> : null}
      <span className="[overflow-wrap:anywhere]">{text}</span>
    </p>
  );
}

/** What a required question that is not answered yet needs: next to the control, and linked to it. */
function Needs({ id, text }: { id: string; text: string }) {
  return (
    <p id={id} data-slot="choices-need" className="text-xs text-muted-foreground">
      {text}
    </p>
  );
}

/** The legend, help and group of a question answered by radio buttons or a list. */
function Group({
  question,
  radiogroup,
  children,
  enabled,
  shown,
  need,
  onClear,
}: {
  question: ChoiceQuestion;
  radiogroup: boolean;
  children: ReactNode;
  enabled: boolean;
  shown?: readonly ReactNode[] | undefined;
  /** What a required question that has no answer yet needs, said next to it and linked to it. */
  need?: string | undefined;
  /** An optional question that has an answer can be cleared. */
  onClear?: (() => void) | undefined;
}) {
  const legend = useId();
  const help = useId();
  const needId = useId();
  const describedBy = [question.help ? help : "", need ? needId : ""].filter(Boolean).join(" ");
  return (
    <fieldset
      data-slot="choices-question"
      data-kind={question.type}
      role={radiogroup ? "radiogroup" : undefined}
      aria-labelledby={legend}
      aria-describedby={describedBy || undefined}
      disabled={groupDisabled(enabled, shown)}
      className="m-0 flex min-w-0 flex-col gap-2 border-0 p-0"
    >
      <legend id={legend} className="mb-1 p-0 text-sm font-medium [overflow-wrap:anywhere]">
        {question.question}
        {optional(question)}
      </legend>
      {question.help ? (
        <p id={help} className="-mt-1 text-xs text-muted-foreground [overflow-wrap:anywhere]">
          {question.help}
        </p>
      ) : null}
      <Shown nodes={shown} />
      <Controls enabled={enabled} nodes={shown}>
        {children}
      </Controls>
      {need && enabled ? <Needs id={needId} text={need} /> : null}
      {onClear && enabled ? (
        <div>
          <Button
            type="button"
            variant="ghost"
            size="sm"
            data-slot="choices-clear"
            aria-label={`Clear answer to ${question.question}`}
            onClick={onClear}
          >
            Clear answer
          </Button>
        </div>
      ) : null}
    </fieldset>
  );
}

/** One radio button of a yes or no, or a point of a scale; the whole tile is the target. */
function RadioTile({
  name,
  value,
  label,
  note,
  fill = false,
  checked,
  onPick,
}: {
  name: string;
  value: string;
  label: string;
  /** The words at an end of a scale, said to a screen reader as the description. */
  note?: string | undefined;
  /** The tile takes the whole width it is given (a yes or no), instead of the width of its words. */
  fill?: boolean;
  checked: boolean;
  onPick: () => void;
}) {
  const id = useId();
  const described = `${id}-n`;
  return (
    <div className={cn("flex min-w-0 flex-col gap-1", fill ? "items-stretch" : "items-center")}>
      <div data-slot="choices-option" className={`${TILE} justify-center`}>
        <input
          id={id}
          type="radio"
          name={name}
          value={value}
          checked={checked}
          onChange={onPick}
          aria-describedby={note ? described : undefined}
          className={RADIO}
        />
        <label
          htmlFor={id}
          className="cursor-pointer [overflow-wrap:anywhere] after:absolute after:inset-0 after:content-['']"
        >
          {label}
        </label>
      </div>
      {note ? (
        <span
          id={described}
          data-slot="choices-scale-end"
          className="max-w-28 text-center text-xs text-muted-foreground [overflow-wrap:anywhere]"
        >
          {note}
        </span>
      ) : null}
    </div>
  );
}

function Confirm({ question, name, state, enabled, onChange, shown }: Props) {
  const chosen = state.values[0];
  const pick = (value: string) => () => onChange({ values: [value] });
  return (
    <Group
      question={question}
      radiogroup
      enabled={enabled}
      shown={shown}
      need={question.required && chosen === undefined ? "Choose yes or no." : undefined}
      onClear={
        !question.required && chosen !== undefined ? () => onChange({ values: [] }) : undefined
      }
    >
      <div className="grid grid-cols-2 gap-2">
        <RadioTile
          name={name}
          value={YES}
          fill
          label={yesLabel(question)}
          checked={chosen === YES}
          onPick={pick(YES)}
        />
        <RadioTile
          name={name}
          value={NO}
          fill
          label={noLabel(question)}
          checked={chosen === NO}
          onPick={pick(NO)}
        />
      </div>
    </Group>
  );
}

function Scale({ question, name, state, enabled, onChange, shown }: Props) {
  const points = scalePoints(question);
  const first = points[0];
  const last = points.at(-1);
  return (
    <Group
      question={question}
      radiogroup
      enabled={enabled}
      shown={shown}
      need={
        question.required && state.values.length === 0
          ? `Choose a point from ${first} to ${last}.`
          : undefined
      }
      onClear={
        !question.required && state.values.length > 0 ? () => onChange({ values: [] }) : undefined
      }
    >
      <div data-slot="choices-scale" className="flex flex-wrap items-start gap-2">
        {points.map((point) => {
          const value = String(point);
          return (
            <RadioTile
              key={value}
              name={name}
              value={value}
              label={value}
              note={
                point === first ? question.fromLabel : point === last ? question.toLabel : undefined
              }
              checked={state.values[0] === value}
              onPick={() => onChange({ values: [value] })}
            />
          );
        })}
      </div>
    </Group>
  );
}

/** What a number's bounds and step say, for the person who is about to type one. */
function numberHint(q: ChoiceQuestion): string | undefined {
  const unit = q.unit ? ` ${q.unit}` : "";
  const parts: string[] = [];
  if (q.minValue !== undefined && q.maxValue !== undefined) {
    parts.push(`From ${q.minValue} to ${q.maxValue}${unit}.`);
  } else if (q.minValue !== undefined) {
    parts.push(`At least ${q.minValue}${unit}.`);
  } else if (q.maxValue !== undefined) {
    parts.push(`At most ${q.maxValue}${unit}.`);
  }
  if (q.step !== undefined) parts.push(`In steps of ${q.step}.`);
  return parts.length > 0 ? parts.join(" ") : undefined;
}

/** A question the person types or picks as one string: a number (a text box) or a day (a date control). */
function Typed({ question, state, enabled, onChange, shown }: Props) {
  const id = useId();
  const labelId = `${id}-label`;
  const notes = `${id}-notes`;
  const problem = `${id}-problem`;
  const needId = `${id}-need`;
  const needed = question.required && state.text.trim() === "" && enabled;
  const number = question.type === "number";
  const text = state.text;
  // an empty box is "not answered yet" (the form says so); a box with something in it is judged now
  const failure = text.trim() === "" ? undefined : checkValues(question, valuesOf(question, state));
  const hint = number ? numberHint(question) : undefined;
  const help = [question.help, hint].filter(Boolean).join(" ");
  return (
    <div
      data-slot="choices-question"
      data-kind={question.type}
      // a typed question has no group of its own; with something shown it is one, named by its words
      {...groupOf(shown, labelId)}
      className="flex min-w-0 flex-col gap-1.5"
    >
      <label id={labelId} htmlFor={id} className="text-sm font-medium [overflow-wrap:anywhere]">
        {question.question}
        {question.unit ? (
          <span className="font-normal text-muted-foreground"> (in {question.unit})</span>
        ) : null}
        {optional(question)}
      </label>
      {help ? (
        <p id={notes} className="text-xs text-muted-foreground [overflow-wrap:anywhere]">
          {help}
        </p>
      ) : null}
      <Shown nodes={shown} />
      <div className="flex items-center gap-2">
        {number ? (
          <Input
            id={id}
            type="text"
            inputMode="decimal"
            autoComplete="off"
            spellCheck={false}
            value={text}
            maxLength={32}
            disabled={!enabled}
            aria-invalid={failure ? true : undefined}
            aria-describedby={[help ? notes : "", problem, needed ? needId : ""]
              .filter(Boolean)
              .join(" ")}
            onChange={(e) => onChange({ text: e.target.value })}
            className="max-w-48"
          />
        ) : (
          <Input
            id={id}
            type="date"
            value={text}
            min={question.minDate}
            // a browser's date box takes a six-digit year unless told; the contract's years are four digits
            max={question.maxDate ?? "9999-12-31"}
            disabled={!enabled}
            aria-invalid={failure ? true : undefined}
            aria-describedby={[help ? notes : "", problem, needed ? needId : ""]
              .filter(Boolean)
              .join(" ")}
            onChange={(e) => onChange({ text: e.target.value })}
            className="max-w-48"
          />
        )}
        {number && question.unit ? (
          <span aria-hidden="true" className="text-sm text-muted-foreground">
            {question.unit}
          </span>
        ) : null}
      </div>
      <Problem id={problem} text={failure?.message} />
      {needed ? <Needs id={needId} text={number ? "Enter a number." : "Pick a day."} /> : null}
    </div>
  );
}

/** The order the list is shown in: the person's, else the agent's. */
const orderOf = (q: ChoiceQuestion, s: QuestionState): string[] =>
  s.values.length === q.options.length
    ? s.values
    : q.defaults.length === q.options.length
      ? q.defaults // the order the agent (or the person's earlier answer) starts the list in
      : q.options.map((o) => o.value);

function Rank({ question, state, enabled, onChange, shown }: Props) {
  const order = orderOf(question, state);
  const touched = state.values.length > 0;
  const [said, setSaid] = useState("");
  const spoken = useRef(0);
  // the same words twice are not announced again: every other message ends in a no-break space
  const say = (text: string) => {
    spoken.current += 1;
    setSaid(spoken.current % 2 === 0 ? `${text}\u00a0` : text);
  };
  // after a move the keyboard stays on the button it used, whichever place the item is now in
  const keep = useRef<string | undefined>(undefined);
  const list = useRef<HTMLOListElement>(null);
  useLayoutEffect(() => {
    if (keep.current === undefined) return;
    const want = keep.current;
    keep.current = undefined;
    for (const el of list.current?.querySelectorAll<HTMLElement>("[data-move]") ?? []) {
      if (el.dataset.move === want) el.focus();
    }
  });

  const move = (value: string, by: -1 | 1) => {
    const from = order.indexOf(value);
    const to = from + by;
    const name = question.options.find((o) => o.value === value)?.label ?? value;
    if (to < 0 || to >= order.length) {
      // a button at an end of the list does nothing, and says so in words
      say(`${name} is already ${by < 0 ? "first" : "last"}`);
      return;
    }
    const next = [...order];
    next.splice(from, 1);
    next.splice(to, 0, value);
    keep.current = `${value}:${by < 0 ? "up" : "down"}`;
    say(`${name}, ${to + 1} of ${order.length}`);
    onChange({ values: next });
  };

  return (
    <Group
      question={question}
      radiogroup={false}
      enabled={enabled}
      shown={shown}
      need={
        question.required && !touched ? "Move an item, or keep the order, to answer." : undefined
      }
      onClear={!question.required && touched ? () => onChange({ values: [] }) : undefined}
    >
      <ol
        ref={list}
        data-slot="choices-rank"
        aria-label={`${question.question}: the first is the best`}
        // biome-ignore lint/a11y/noRedundantRoles: list-none drops the list for VoiceOver (Safari) unless the role is said
        role="list"
        className="m-0 flex list-none flex-col gap-2 p-0"
      >
        {order.map((value, index) => {
          const option = question.options.find((o) => o.value === value);
          if (!option) return null;
          const Icon =
            option.icon && option.icon in STEP_ICON
              ? STEP_ICON[option.icon as StepIcon]
              : undefined;
          const atTop = index === 0;
          const atEnd = index === order.length - 1;
          return (
            <li
              key={value}
              data-slot="choices-rank-item"
              className="flex min-h-11 items-center gap-2 rounded-lg border bg-background px-3 py-2 text-sm"
            >
              <span aria-hidden="true" className="w-5 shrink-0 text-muted-foreground tabular-nums">
                {index + 1}.
              </span>
              <span className="flex min-w-0 flex-1 flex-col">
                <span className="flex flex-wrap items-center gap-x-2 gap-y-1">
                  {Icon ? (
                    <Icon aria-hidden="true" className="size-4 shrink-0 text-muted-foreground" />
                  ) : null}
                  <span data-slot="choices-rank-label" className="[overflow-wrap:anywhere]">
                    {option.label}
                  </span>
                  {option.recommended ? (
                    <Badge variant="secondary" data-slot="choices-recommended">
                      Recommended
                    </Badge>
                  ) : null}
                </span>
                {option.description ? (
                  <span className="text-xs text-muted-foreground [overflow-wrap:anywhere]">
                    {option.description}
                  </span>
                ) : null}
              </span>
              <Button
                type="button"
                variant="outline"
                size="icon"
                data-move={`${value}:up`}
                aria-label={`Move ${option.label} up`}
                // not `disabled` at an end: a disabled button drops the keyboard's place
                aria-disabled={atTop || undefined}
                disabled={!enabled}
                onClick={() => move(value, -1)}
                className={`size-11 ${atTop ? "opacity-50" : ""}`}
              >
                <ArrowUpIcon aria-hidden="true" />
              </Button>
              <Button
                type="button"
                variant="outline"
                size="icon"
                data-move={`${value}:down`}
                aria-label={`Move ${option.label} down`}
                aria-disabled={atEnd || undefined}
                disabled={!enabled}
                onClick={() => move(value, 1)}
                className={`size-11 ${atEnd ? "opacity-50" : ""}`}
              >
                <ArrowDownIcon aria-hidden="true" />
              </Button>
            </li>
          );
        })}
      </ol>
      <p role="status" className="sr-only">
        {said}
      </p>
      {touched ? null : (
        <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
          <Button
            type="button"
            variant="outline"
            size="sm"
            disabled={!enabled}
            onClick={() => onChange({ values: order })}
          >
            Keep this order
          </Button>
        </div>
      )}
    </Group>
  );
}

/** A question of one of the five kinds of version 7. */
export function KindQuestion(props: Props): ReactNode {
  switch (props.question.type) {
    case "confirm":
      return <Confirm {...props} />;
    case "scale":
      return <Scale {...props} />;
    case "number":
    case "date":
      return <Typed {...props} />;
    case "rank":
      return <Rank {...props} />;
    default:
      return null;
  }
}
