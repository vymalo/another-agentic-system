"use client";

import { CheckIcon, ChevronsUpDownIcon, ImageOffIcon, XIcon } from "lucide-react";
import { type KeyboardEvent, type ReactNode, useId, useState } from "react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { FileImage } from "@/features/chat/components/cards/kept-file-card";
import { STEP_ICON } from "@/features/chat/components/steps/step-icons";
import {
  type AnswerState,
  answerContext,
  type ChoiceOption,
  type ChoiceQuestion,
  chosenCount,
  EMPTY,
  initialState,
  isTyping,
  MAX_LONG_TEXT,
  MAX_NOTE,
  MAX_OTHER,
  MAX_TEXT,
  missing,
  type QuestionState,
  readChoices,
} from "@/features/chat/lib/a2ui/choices";
import { contextTooLarge } from "@/features/chat/lib/a2ui/context";
import { MAX_CONTEXT_BYTES } from "@/features/chat/lib/a2ui/limits";
import type { StepIcon } from "@/features/chat/lib/agui/vymalo";
import { cn } from "@/lib/utils";
import { useSurfaceHost } from "./surface-host";
import { useThreadFiles } from "./thread-files";
import { useView } from "./view-context";

/*
 * The `Choices` component of the UI catalog (docs/api/ui-catalog-v1.md, version 5): a form of up to
 * 8 questions with a title and a description of why the agent asks, and per question a type: radio
 * buttons, check boxes, a searchable list, a deck of pictures, or a typed answer; optionally an
 * "Other" of the person's own words per question and a free box for the whole form. One submit sends
 * ONE action, and nothing sends but that click (ADR 0013 rule 7): Enter in a text box does nothing,
 * because there is no form.
 *
 * What reaches this file has passed the schema of the catalog and `prepareSurface`; the strings
 * are the agent's, drawn as React text, and a picture is a file of the thread, drawn from the API's
 * own route. The choices are native inputs in a `fieldset`: the browser gives the arrow keys, the
 * roving tab stop and the screen reader's group for free, and each question's group is named by its
 * legend. The searchable list is the combobox of ARIA 1.2: the focus stays in the text box and the
 * arrow keys move the active option.
 */

const TILE =
  "relative flex min-h-11 gap-3 rounded-lg border bg-background px-3 py-2.5 text-sm transition-colors " +
  "has-[:checked]:border-brand has-[:checked]:bg-brand/5 " +
  "has-[:focus-visible]:ring-3 has-[:focus-visible]:ring-ring/50 " +
  "has-[:disabled]:cursor-not-allowed has-[:disabled]:opacity-60";
const DECK_TILE =
  "relative flex min-w-0 flex-col gap-2 rounded-lg border bg-background p-2 text-sm transition-colors " +
  "has-[:checked]:border-brand has-[:checked]:bg-brand/5 " +
  "has-[:focus-visible]:ring-3 has-[:focus-visible]:ring-ring/50 " +
  "has-[:disabled]:cursor-not-allowed has-[:disabled]:opacity-60";
const CONTROL = "mt-0.5 size-4 shrink-0 accent-brand disabled:cursor-not-allowed";
const TEXTAREA =
  "min-h-16 w-full min-w-0 rounded-md border border-input bg-transparent px-2.5 py-1.5 text-base shadow-xs outline-none placeholder:text-muted-foreground focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50 disabled:cursor-not-allowed disabled:opacity-50 md:text-sm dark:bg-input/30";

type Patch = (patch: Partial<QuestionState>) => void;
type ListType = "radio" | "checkbox";

const iconOf = (option: ChoiceOption) =>
  option.icon && option.icon in STEP_ICON ? STEP_ICON[option.icon as StepIcon] : undefined;

/** The picture of a deck tile: a file of the thread, or a line saying it is not there. */
function DeckPicture({ option }: { option: ChoiceOption }) {
  const files = useThreadFiles();
  const file = option.image ? files.find((f) => f.sha256 === option.image?.sha256) : undefined;
  if (file?.preview !== "image") {
    return (
      <p
        data-slot="choices-picture-missing"
        className="flex aspect-4/3 items-center justify-center gap-2 rounded-md border border-dashed px-2 text-xs text-muted-foreground"
      >
        <ImageOffIcon aria-hidden="true" className="size-4 shrink-0" />
        <span className="[overflow-wrap:anywhere]">{option.image?.alt ?? "Image not shown"}</span>
      </p>
    );
  }
  return (
    <FileImage
      file={file}
      alt={option.image?.alt}
      className="aspect-4/3 w-full rounded-md border bg-muted/30 object-cover"
    />
  );
}

/** One option: the whole tile is the click target (the label's `::after` covers it). */
function OptionTile({
  option,
  type,
  name,
  checked,
  disabled = false,
  deck = false,
  onChange,
}: {
  option: ChoiceOption;
  type: ListType;
  name: string;
  checked: boolean;
  disabled?: boolean;
  deck?: boolean;
  onChange: () => void;
}) {
  const id = useId();
  const described = `${id}-d`;
  const badge = `${id}-r`;
  const Icon = iconOf(option);
  const describedBy =
    [option.description ? described : "", option.recommended ? badge : ""]
      .filter(Boolean)
      .join(" ") || undefined;
  return (
    <div
      data-slot={deck ? "choices-deck-option" : "choices-option"}
      className={deck ? DECK_TILE : TILE}
    >
      {deck ? <DeckPicture option={option} /> : null}
      <div className="flex min-w-0 gap-3">
        <input
          id={id}
          type={type}
          name={name}
          value={option.value}
          checked={checked}
          disabled={disabled && !checked}
          onChange={onChange}
          aria-describedby={describedBy}
          className={CONTROL}
        />
        <span className="flex min-w-0 flex-col">
          <span className="flex flex-wrap items-center gap-x-2 gap-y-1">
            <label
              htmlFor={id}
              className="flex cursor-pointer items-center gap-1.5 [overflow-wrap:anywhere] after:absolute after:inset-0 after:content-['']"
            >
              {Icon ? (
                <Icon aria-hidden="true" className="size-4 shrink-0 text-muted-foreground" />
              ) : null}
              {option.label}
            </label>
            {option.recommended ? (
              <Badge id={badge} variant="secondary" data-slot="choices-recommended">
                Recommended
              </Badge>
            ) : null}
          </span>
          {option.description ? (
            <span id={described} className="text-xs text-muted-foreground [overflow-wrap:anywhere]">
              {option.description}
            </span>
          ) : null}
        </span>
      </div>
    </div>
  );
}

/** "Other": a choice of its own and the person's words; typing in the box turns it on. */
function OtherTile({
  question,
  type,
  name,
  state,
  disabled = false,
  onChange,
}: {
  question: ChoiceQuestion;
  type: ListType;
  name: string;
  state: QuestionState;
  disabled?: boolean;
  onChange: Patch;
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
        disabled={disabled && !state.otherOn}
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
        disabled={disabled && !state.otherOn}
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

/** The options a question has with `value` chosen (or not), in the way its type allows. */
function pick(
  question: ChoiceQuestion,
  state: QuestionState,
  value: string,
): Partial<QuestionState> {
  if (!question.multiple) return { values: [value], otherOn: false };
  return {
    values: state.values.includes(value)
      ? state.values.filter((v) => v !== value)
      : [...state.values, value],
  };
}

/** The searchable list: the text box filters, the arrows move, Enter chooses, Escape closes. */
function SelectBox({
  question,
  state,
  atMax,
  labelledBy,
  describedBy,
  onChange,
}: {
  question: ChoiceQuestion;
  state: QuestionState;
  atMax: boolean;
  labelledBy: string;
  describedBy: string | undefined;
  onChange: Patch;
}) {
  const id = useId();
  const list = `${id}-list`;
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [active, setActive] = useState(0);
  const needle = query.trim().toLowerCase();
  const shown = question.options.filter(
    (o) => needle === "" || `${o.label} ${o.description ?? ""}`.toLowerCase().includes(needle),
  );
  const current = question.multiple
    ? undefined
    : question.options.find((o) => o.value === state.values[0]);
  const chosen = question.options.filter((o) => state.values.includes(o.value));

  const choose = (option: ChoiceOption) => {
    if (question.multiple && atMax && !state.values.includes(option.value)) return;
    onChange(pick(question, state, option.value));
    if (!question.multiple) setOpen(false);
    setQuery("");
  };
  const activeOption = open ? shown[Math.min(active, shown.length - 1)] : undefined;
  const optionId = (value: string) =>
    `${id}-o-${question.options.findIndex((o) => o.value === value)}`;

  const onKeyDown = (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      if (!open) {
        setOpen(true);
        return;
      }
      const step = e.key === "ArrowDown" ? 1 : -1;
      setActive((a) =>
        Math.max(0, Math.min(shown.length - 1, Math.min(a, shown.length - 1) + step)),
      );
    } else if (e.key === "Enter" && open) {
      // chooses the active option; it never sends the form (there is none)
      e.preventDefault();
      if (activeOption) choose(activeOption);
    } else if (e.key === "Escape" && open) {
      e.preventDefault();
      e.stopPropagation();
      setOpen(false);
    }
  };

  return (
    <div data-slot="choices-select" className="flex min-w-0 flex-col gap-2">
      {question.multiple && chosen.length > 0 ? (
        <ul data-slot="choices-chips" className="m-0 flex list-none flex-wrap gap-1.5 p-0">
          {chosen.map((o) => (
            <li key={o.value}>
              <Badge variant="secondary" className="h-6 gap-1 pr-1">
                <span className="[overflow-wrap:anywhere]">{o.label}</span>
                <button
                  type="button"
                  aria-label={`Remove ${o.label}`}
                  onClick={() => onChange(pick(question, state, o.value))}
                  className="rounded-full p-0.5 hover:bg-foreground/10 disabled:cursor-not-allowed"
                >
                  <XIcon aria-hidden="true" className="size-3" />
                </button>
              </Badge>
            </li>
          ))}
        </ul>
      ) : null}
      <div className="relative">
        <Input
          role="combobox"
          aria-expanded={open}
          aria-controls={list}
          aria-autocomplete="list"
          aria-labelledby={labelledBy}
          aria-describedby={describedBy}
          aria-activedescendant={activeOption ? optionId(activeOption.value) : undefined}
          autoComplete="off"
          placeholder={question.multiple ? "Search and choose…" : "Search…"}
          value={open || question.multiple ? query : (current?.label ?? "")}
          onChange={(e) => {
            setQuery(e.target.value);
            setActive(0);
            setOpen(true);
          }}
          onFocus={() => setOpen(true)}
          onClick={() => setOpen(true)}
          onBlur={() => {
            setOpen(false);
            setQuery("");
          }}
          onKeyDown={onKeyDown}
          className="pr-8"
        />
        <ChevronsUpDownIcon
          aria-hidden="true"
          className="pointer-events-none absolute top-1/2 right-2.5 size-4 -translate-y-1/2 text-muted-foreground"
        />
      </div>
      {open ? (
        <div
          id={list}
          role="listbox"
          tabIndex={-1}
          aria-labelledby={labelledBy}
          aria-multiselectable={question.multiple || undefined}
          data-slot="choices-listbox"
          className="max-h-60 overflow-auto rounded-md border bg-background p-1 shadow-xs"
          // keeps the focus in the text box: a click on the list (or its scroll bar) must not blur it
          onMouseDown={(e) => e.preventDefault()}
        >
          {shown.length === 0 ? (
            <p className="m-0 px-2 py-1.5 text-sm text-muted-foreground">No match</p>
          ) : (
            shown.map((o, i) => {
              const selected = state.values.includes(o.value);
              const Icon = iconOf(o);
              const blocked = question.multiple && atMax && !selected;
              return (
                <div
                  key={o.value}
                  id={optionId(o.value)}
                  role="option"
                  tabIndex={-1}
                  aria-selected={selected}
                  aria-disabled={blocked || undefined}
                  data-active={o === activeOption || undefined}
                  onMouseMove={() => setActive(i)}
                  onClick={() => choose(o)}
                  // the focus stays in the text box, whose keys choose; these are for a screen reader's cursor
                  onKeyDown={(e) => {
                    if (e.key === "Enter" || e.key === " ") {
                      e.preventDefault();
                      choose(o);
                    }
                  }}
                  className={cn(
                    "flex cursor-pointer items-start gap-2 rounded-sm px-2 py-1.5 text-sm",
                    o === activeOption && "bg-accent",
                    blocked && "cursor-not-allowed opacity-60",
                  )}
                >
                  <CheckIcon
                    aria-hidden="true"
                    className={cn("mt-0.5 size-4 shrink-0", selected ? "text-brand" : "invisible")}
                  />
                  <span className="flex min-w-0 flex-col">
                    <span className="flex flex-wrap items-center gap-x-2 gap-y-1">
                      {Icon ? (
                        <Icon
                          aria-hidden="true"
                          className="size-4 shrink-0 text-muted-foreground"
                        />
                      ) : null}
                      <span className="[overflow-wrap:anywhere]">{o.label}</span>
                      {o.recommended ? (
                        <Badge variant="secondary" data-slot="choices-recommended">
                          Recommended
                        </Badge>
                      ) : null}
                    </span>
                    {o.description ? (
                      <span className="text-xs text-muted-foreground [overflow-wrap:anywhere]">
                        {o.description}
                      </span>
                    ) : null}
                  </span>
                </div>
              );
            })
          )}
        </div>
      ) : null}
    </div>
  );
}

/** A `text` or `longText` question: a box for the person's words. */
function TypedAnswer({
  question,
  state,
  enabled,
  onChange,
  describedBy,
}: {
  question: ChoiceQuestion;
  state: QuestionState;
  enabled: boolean;
  onChange: Patch;
  describedBy: string | undefined;
}) {
  const id = useId();
  const long = question.type === "longText";
  const props = {
    id,
    value: state.text,
    disabled: !enabled,
    "aria-describedby": describedBy,
    maxLength: long ? MAX_LONG_TEXT : MAX_TEXT,
    onChange: (e: { target: { value: string } }) => onChange({ text: e.target.value }),
  };
  return (
    <div data-slot="choices-question" className="flex min-w-0 flex-col gap-1.5">
      <label htmlFor={id} className="text-sm font-medium [overflow-wrap:anywhere]">
        {question.question}
        {question.required ? null : (
          <span className="font-normal text-muted-foreground"> (optional)</span>
        )}
      </label>
      {question.help ? (
        <p id={describedBy} className="text-xs text-muted-foreground [overflow-wrap:anywhere]">
          {question.help}
        </p>
      ) : null}
      {long ? <textarea {...props} rows={4} className={TEXTAREA} /> : <Input {...props} />}
    </div>
  );
}

/** What a question that takes several choices asks for, in words. */
function rangeHint(q: ChoiceQuestion): string | undefined {
  if (!q.multiple) return undefined;
  const { min, max } = q;
  if (min !== undefined && max !== undefined) {
    return min === max ? `Choose ${min}.` : `Choose ${min} to ${max}.`;
  }
  if (min !== undefined && min > 0) return `Choose at least ${min}.`;
  if (max !== undefined) return `Choose up to ${max}.`;
  return undefined;
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
  onChange: Patch;
}) {
  const legend = useId();
  const notes = useId();
  const typed = isTyping(question.type);
  const range = rangeHint(question);
  const hasNotes = Boolean(question.help || range);
  if (typed) {
    return (
      <TypedAnswer
        question={question}
        state={state}
        enabled={enabled}
        onChange={onChange}
        describedBy={question.help ? `${notes}-help` : undefined}
      />
    );
  }
  const type: ListType = question.multiple ? "checkbox" : "radio";
  const atMax = question.max !== undefined && chosenCount(question, state) >= question.max;
  const radios = question.type === "single" || (question.type === "images" && !question.multiple);
  const toggle = (value: string) => onChange(pick(question, state, value));
  const deck = question.type === "images";
  return (
    <fieldset
      data-slot="choices-question"
      // a radiogroup for the radio buttons, a plain group for the rest: both named by the legend
      role={radios ? "radiogroup" : undefined}
      aria-labelledby={legend}
      aria-describedby={hasNotes ? notes : undefined}
      disabled={!enabled}
      className="m-0 flex min-w-0 flex-col gap-2 border-0 p-0"
    >
      <legend id={legend} className="mb-1 p-0 text-sm font-medium [overflow-wrap:anywhere]">
        {question.question}
        {question.required ? null : (
          <span className="font-normal text-muted-foreground"> (optional)</span>
        )}
      </legend>
      {hasNotes ? (
        <div id={notes} className="-mt-1 flex flex-col gap-0.5">
          {question.help ? (
            <p className="text-xs text-muted-foreground [overflow-wrap:anywhere]">
              {question.help}
            </p>
          ) : null}
          {range ? <p className="text-xs text-muted-foreground">{range}</p> : null}
        </div>
      ) : null}
      {question.type === "select" ? (
        <SelectBox
          question={question}
          state={state}
          atMax={atMax}
          labelledBy={legend}
          describedBy={hasNotes ? notes : undefined}
          onChange={onChange}
        />
      ) : deck ? (
        <div data-slot="choices-deck" className="grid grid-cols-2 gap-2 sm:grid-cols-3">
          {question.options.map((option) => (
            <OptionTile
              key={option.value}
              option={option}
              type={type}
              name={name}
              deck
              checked={state.values.includes(option.value)}
              disabled={atMax}
              onChange={() => toggle(option.value)}
            />
          ))}
        </div>
      ) : (
        question.options.map((option) => (
          <OptionTile
            key={option.value}
            option={option}
            type={type}
            name={name}
            checked={state.values.includes(option.value)}
            disabled={atMax}
            onChange={() => toggle(option.value)}
          />
        ))
      )}
      {question.allowOther ? (
        <OtherTile
          question={question}
          type={type}
          name={name}
          state={state}
          disabled={atMax}
          onChange={onChange}
        />
      ) : null}
    </fieldset>
  );
}

/** The free box of a form: the person's words beyond the questions, sent as the answer's `note`. */
function FreeBox({
  label,
  placeholder,
  value,
  enabled,
  onChange,
}: {
  label: string;
  placeholder: string | undefined;
  value: string;
  enabled: boolean;
  onChange: (value: string) => void;
}) {
  const id = useId();
  return (
    <div data-slot="choices-free-input" className="flex min-w-0 flex-col gap-1.5">
      <label htmlFor={id} className="text-sm font-medium [overflow-wrap:anywhere]">
        {label}
      </label>
      <textarea
        id={id}
        rows={3}
        value={value}
        maxLength={MAX_NOTE}
        disabled={!enabled}
        placeholder={placeholder}
        onChange={(e) => onChange(e.target.value)}
        className={TEXTAREA}
      />
    </div>
  );
}

/** The Choices of a surface; the props are those of the lowered component (`prepare.ts`). */
export function ChoicesInput(props: Record<string, unknown>): ReactNode {
  const view = useView();
  const host = useSurfaceHost();
  const base = useId();
  const spec = readChoices(props);
  const [state, setState] = useState<AnswerState>(() => (spec ? initialState(spec) : new Map()));
  const [note, setNote] = useState("");
  if (!spec) return null;

  // the answers go to the agent while the thread waits for them, from the newest copy only; once
  // sent (or in a thread that moved on) the choices stay as they were, and stop taking input
  const enabled = view.live && host.canSend;
  const left = missing(spec, state);
  const patch = (id: string, p: Partial<QuestionState>) =>
    setState((s) => new Map(s).set(id, { ...(s.get(id) ?? EMPTY), ...p }));

  const send = () => {
    if (!enabled || left > 0) return;
    const context = answerContext(spec, state, note);
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
      {spec.title || spec.description ? (
        <div className="flex min-w-0 flex-col gap-1">
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
          {spec.description ? (
            <p
              data-slot="choices-description"
              className="text-sm whitespace-pre-wrap text-muted-foreground [overflow-wrap:anywhere]"
            >
              {spec.description}
            </p>
          ) : null}
        </div>
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
      {spec.freeInput ? (
        <FreeBox
          label={spec.freeInput.label}
          placeholder={spec.freeInput.placeholder}
          value={note}
          enabled={enabled}
          onChange={setNote}
        />
      ) : null}
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
