"use client";

import { ArrowDownIcon, ArrowUpIcon, PlusIcon, Trash2Icon } from "lucide-react";
import { useEffect, useState } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { NativeSelect, NativeSelectOption } from "@/components/ui/native-select";
import {
  dropsHiddenMembers,
  EDIT_KINDS,
  type EditableQuestion,
  type EditKind,
  type Fields,
  hasHiddenMembers,
  idFromWords,
  KIND_LABEL,
  takesOptions,
} from "../lib/editor";

/** The most options a question of one or several choices holds (docs/api/prompts-v1.md, "The variables"). */
const MAX_OPTIONS = 12;

/** The control of a question's card that a refused member of it is about (`variables.<i>.<member>`); null: the card as a whole. */
export function controlSuffix(path: readonly string[]): string | null {
  const [head, j] = path;
  switch (head) {
    case "id":
      return "id";
    case "question":
      return "question";
    case "help":
      return "help";
    case "type":
      return "kind";
    case "options":
      return j !== undefined && /^\d+$/.test(j) ? `opt-${j}` : "opt-0";
    case "allowOther":
      return "other";
    case "yesLabel":
      return "yes";
    case "noLabel":
      return "no";
    case "minValue":
      return "min";
    case "maxValue":
      return "max";
    case "step":
      return "step";
    case "unit":
      return "unit";
    case "minDate":
      return "mindate";
    case "maxDate":
      return "maxdate";
    case "default":
      return "start";
    default:
      return null;
  }
}

type Props = {
  q: EditableQuestion;
  index: number;
  count: number;
  /** `<form id>-<question key>`: the ids of the card's controls start with it. */
  baseId: string;
  /** A save is pending: the cards are not rearranged under it. */
  busy: boolean;
  /** The ids of the other questions, so an id made from the words is not one of them. */
  otherIds: readonly string[];
  /** What the server refused about this question, and the control it is about (`controlSuffix`). */
  refusal: { message: string; suffix: string | null } | null;
  onChange: (next: EditableQuestion) => void;
  onMove: (delta: -1 | 1) => void;
  onRemove: () => void;
  /** Writes `{{id}}` into the text where the caret last was. */
  onInsert: (id: string) => void;
};

const joined = (...ids: (string | false | undefined)[]): string | undefined => {
  const out = ids.filter(Boolean).join(" ");
  return out === "" ? undefined : out;
};

/**
 * One question of a prompt, in the seven kinds this page edits. Every control is native, labelled, and tied to its help
 * line and to the server's refusal of it (`aria-invalid`, `aria-describedby`); questions are reordered with buttons.
 * Members the page does not show are not here and are not lost (`lib/editor.ts`).
 */
export function QuestionEditor({
  q,
  index,
  count,
  baseId,
  busy,
  otherIds,
  refusal,
  onChange,
  onMove,
  onRemove,
  onInsert,
}: Props) {
  const f = q.fields;
  const n = index + 1;
  const errorId = `${baseId}-error`;
  const set = (patch: Partial<Fields>, extra: Partial<EditableQuestion> = {}) =>
    onChange({ ...q, ...extra, fields: { ...f, ...patch } });
  const bad = (suffix: string) => refusal?.suffix === suffix;
  const tie = (suffix: string, help?: string) => joined(help, bad(suffix) && errorId);
  const invalid = (suffix: string) => (bad(suffix) ? true : undefined);

  function setKind(kind: EditKind) {
    let options = f.options;
    while (takesOptions(kind) && options.length < 2) {
      options = [...options, { value: "", label: "", raw: null }];
    }
    set({ kind, options });
  }

  const setOption = (j: number, label: string) =>
    set({ options: f.options.map((o, k) => (k === j ? { ...o, label } : o)) });
  // after a move the focus stays on the same control of the moved option, or on the other direction's when that one is now disabled
  const [moved, setMoved] = useState<{ at: number; dir: -1 | 1; text: string } | null>(null);
  // biome-ignore lint/correctness/useExhaustiveDependencies: `moved` is a new object for every move
  useEffect(() => {
    if (!moved) return;
    const base = `${baseId}-opt-${moved.at}`;
    const same = document.getElementById(`${base}-${moved.dir < 0 ? "up" : "down"}`);
    const other = document.getElementById(`${base}-${moved.dir < 0 ? "down" : "up"}`);
    const pick = same instanceof HTMLButtonElement && !same.disabled ? same : other;
    if (pick instanceof HTMLElement) pick.focus();
  }, [moved]);
  const moveOption = (j: number, delta: -1 | 1) => {
    const options = [...f.options];
    const [item] = options.splice(j, 1);
    if (item) options.splice(j + delta, 0, item);
    set({ options });
    setMoved({
      at: j + delta,
      dir: delta,
      text: `Option moved to position ${j + delta + 1} of ${options.length}`,
    });
  };

  const textField = (
    suffix: string,
    label: string,
    value: string,
    onText: (v: string) => void,
    extra: { help?: string; type?: string; inputMode?: "decimal" } = {},
  ) => (
    <div className="flex min-w-0 flex-col gap-1.5">
      <Label htmlFor={`${baseId}-${suffix}`}>{label}</Label>
      <Input
        id={`${baseId}-${suffix}`}
        value={value}
        {...(extra.type ? { type: extra.type } : {})}
        {...(extra.inputMode ? { inputMode: extra.inputMode } : {})}
        autoComplete="off"
        aria-invalid={invalid(suffix)}
        aria-describedby={tie(suffix, extra.help ? `${baseId}-${suffix}-help` : undefined)}
        onChange={(e) => onText(e.target.value)}
      />
      {extra.help ? (
        <p id={`${baseId}-${suffix}-help`} className="text-xs text-muted-foreground">
          {extra.help}
        </p>
      ) : null}
    </div>
  );

  return (
    <li
      data-slot="question-editor"
      data-question={q.key}
      className="flex flex-col gap-4 rounded-xl border bg-card px-4 py-4 text-card-foreground"
    >
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h3
          id={baseId}
          tabIndex={-1}
          aria-describedby={refusal && refusal.suffix === null ? errorId : undefined}
          className="text-sm font-medium outline-none"
        >
          Question {n}
        </h3>
        <div className="flex gap-1">
          <Button
            type="button"
            variant="ghost"
            size="icon-sm"
            id={`${baseId}-up`}
            aria-label={`Move question ${n} up`}
            disabled={busy || index === 0}
            onClick={() => onMove(-1)}
          >
            <ArrowUpIcon aria-hidden="true" />
          </Button>
          <Button
            type="button"
            variant="ghost"
            size="icon-sm"
            id={`${baseId}-down`}
            aria-label={`Move question ${n} down`}
            disabled={busy || index === count - 1}
            onClick={() => onMove(1)}
          >
            <ArrowDownIcon aria-hidden="true" />
          </Button>
          <Button
            type="button"
            variant="ghost"
            size="icon-sm"
            aria-label={`Remove question ${n}`}
            disabled={busy}
            onClick={onRemove}
          >
            <Trash2Icon aria-hidden="true" />
          </Button>
        </div>
      </div>

      {textField(
        "question",
        "Question",
        f.question,
        (v) => {
          // the name in the text follows the first words of the question until the person writes one of their own
          if (!q.idTouched && q.raw === null) {
            set({ question: v, id: idFromWords(v, otherIds, n) });
          } else set({ question: v });
        },
        { help: "What the person is asked when they use the prompt." },
      )}

      <div className="grid gap-4 sm:grid-cols-2">
        <div className="flex min-w-0 flex-col gap-1.5">
          <Label htmlFor={`${baseId}-id`}>Name in the text</Label>
          <Input
            id={`${baseId}-id`}
            value={f.id}
            autoComplete="off"
            autoCapitalize="none"
            spellCheck={false}
            aria-invalid={invalid("id")}
            aria-describedby={tie("id", `${baseId}-id-help`)}
            onChange={(e) => set({ id: e.target.value }, { idTouched: true })}
          />
          <p id={`${baseId}-id-help`} className="text-xs text-muted-foreground">
            Letters, digits and - _ . : only.
          </p>
          <Button
            type="button"
            variant="outline"
            size="sm"
            className="self-start"
            disabled={f.id.trim() === ""}
            onClick={() => onInsert(f.id.trim())}
          >
            Insert {`{{${f.id.trim() || "name"}}}`} into the text
          </Button>
        </div>
        <div className="flex min-w-0 flex-col gap-1.5">
          <Label htmlFor={`${baseId}-kind`}>Kind</Label>
          <NativeSelect
            id={`${baseId}-kind`}
            value={f.kind}
            className="w-full"
            aria-invalid={invalid("kind")}
            aria-describedby={tie("kind")}
            onChange={(e) => setKind(e.target.value as EditKind)}
          >
            {EDIT_KINDS.map((kind) => (
              <NativeSelectOption key={kind} value={kind}>
                {KIND_LABEL[kind]}
              </NativeSelectOption>
            ))}
          </NativeSelect>
        </div>
      </div>

      {textField("help", "Help (optional)", f.help, (v) => set({ help: v }))}

      {takesOptions(f.kind) ? (
        <fieldset className="flex min-w-0 flex-col gap-2">
          <legend className="text-sm font-medium">Options</legend>
          <ol className="flex flex-col gap-2">
            {f.options.map((o, j) => (
              // biome-ignore lint/suspicious/noArrayIndexKey: options have no identity until they are saved; the position is the key
              <li key={j} className="flex min-w-0 items-center gap-1">
                <Input
                  id={`${baseId}-opt-${j}`}
                  value={o.label}
                  autoComplete="off"
                  aria-label={`Option ${j + 1} of question ${n}`}
                  aria-invalid={invalid(`opt-${j}`)}
                  aria-describedby={tie(`opt-${j}`)}
                  onChange={(e) => setOption(j, e.target.value)}
                />
                <Button
                  type="button"
                  variant="ghost"
                  size="icon-sm"
                  id={`${baseId}-opt-${j}-up`}
                  aria-label={`Move option ${j + 1} of question ${n} up`}
                  disabled={busy || j === 0}
                  onClick={() => moveOption(j, -1)}
                >
                  <ArrowUpIcon aria-hidden="true" />
                </Button>
                <Button
                  type="button"
                  variant="ghost"
                  size="icon-sm"
                  id={`${baseId}-opt-${j}-down`}
                  aria-label={`Move option ${j + 1} of question ${n} down`}
                  disabled={busy || j === f.options.length - 1}
                  onClick={() => moveOption(j, 1)}
                >
                  <ArrowDownIcon aria-hidden="true" />
                </Button>
                <Button
                  type="button"
                  variant="ghost"
                  size="icon-sm"
                  aria-label={`Remove option ${j + 1} of question ${n}`}
                  disabled={busy}
                  onClick={() => set({ options: f.options.filter((_, k) => k !== j) })}
                >
                  <Trash2Icon aria-hidden="true" />
                </Button>
              </li>
            ))}
          </ol>
          <p role="status" aria-live="polite" className="sr-only">
            {moved?.text ?? ""}
          </p>
          {f.options.length >= MAX_OPTIONS ? (
            <p className="text-xs text-muted-foreground">
              A question has at most {MAX_OPTIONS} options.
            </p>
          ) : (
            <Button
              type="button"
              variant="outline"
              size="sm"
              className="self-start"
              aria-label={`Add an option to question ${n}`}
              onClick={() => set({ options: [...f.options, { value: "", label: "", raw: null }] })}
            >
              <PlusIcon aria-hidden="true" />
              Add option
            </Button>
          )}
          <div className="flex items-center gap-2">
            <input
              id={`${baseId}-other`}
              type="checkbox"
              className="size-4 accent-brand"
              checked={f.allowOther}
              aria-invalid={invalid("other")}
              aria-describedby={tie("other")}
              onChange={(e) => set({ allowOther: e.target.checked })}
            />
            <Label htmlFor={`${baseId}-other`} className="font-normal">
              Let people write their own answer
            </Label>
          </div>
        </fieldset>
      ) : null}

      {f.kind === "confirm" ? (
        <div className="grid gap-4 sm:grid-cols-2">
          {textField("yes", "Yes label (optional)", f.yesLabel, (v) => set({ yesLabel: v }))}
          {textField("no", "No label (optional)", f.noLabel, (v) => set({ noLabel: v }))}
        </div>
      ) : null}

      {f.kind === "number" ? (
        <div className="grid gap-4 sm:grid-cols-2">
          {textField("min", "Smallest (optional)", f.minValue, (v) => set({ minValue: v }), {
            inputMode: "decimal",
          })}
          {textField("max", "Largest (optional)", f.maxValue, (v) => set({ maxValue: v }), {
            inputMode: "decimal",
          })}
          {textField("step", "Step (optional)", f.step, (v) => set({ step: v }), {
            inputMode: "decimal",
          })}
          {textField("unit", "Unit (optional)", f.unit, (v) => set({ unit: v }))}
        </div>
      ) : null}

      {f.kind === "date" ? (
        <div className="grid gap-4 sm:grid-cols-2">
          {textField("mindate", "Earliest (optional)", f.minDate, (v) => set({ minDate: v }), {
            type: "date",
          })}
          {textField("maxdate", "Latest (optional)", f.maxDate, (v) => set({ maxDate: v }), {
            type: "date",
          })}
        </div>
      ) : null}

      {f.kind === "text" || f.kind === "longText"
        ? textField("start", "Starts with (optional)", f.start, (v) => set({ start: v }))
        : null}

      <div className="flex items-center gap-2">
        <input
          id={`${baseId}-required`}
          type="checkbox"
          className="size-4 accent-brand"
          checked={f.required}
          onChange={(e) => set({ required: e.target.checked })}
        />
        <Label htmlFor={`${baseId}-required`} className="font-normal">
          Required
        </Label>
      </div>

      {hasHiddenMembers(q) ? (
        <p className="text-xs text-muted-foreground">
          This question has settings this page does not show. They are kept.
        </p>
      ) : null}
      {dropsHiddenMembers(q) ? (
        <p className="text-xs text-muted-foreground">
          Changing the kind removes the settings this page does not show.
        </p>
      ) : null}
      {refusal ? (
        <p id={errorId} role="alert" className="text-sm text-destructive">
          {refusal.message}
        </p>
      ) : null}
    </li>
  );
}
