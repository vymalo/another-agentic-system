"use client";

import { type FormEvent, useEffect, useId, useRef, useState } from "react";
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
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { NativeSelect, NativeSelectOption } from "@/components/ui/native-select";
import { RadioGroup, RadioGroupItem } from "@/components/ui/radio-group";
import type { Tier } from "@/features/tier/lib/tier";
import { uuidv7 } from "@/lib/uuid";
import { type StartDraft, useStartSession } from "../hooks/use-start-session";
import { checkStartDescription, codePointLength, MAX_START_DESCRIPTION } from "../lib/description";
import type { ChildSessionsOffer } from "../lib/offer";
import { refusalWords } from "../lib/refusals";

/** The longest prompt, in characters (`ChildSessionStart.text`). */
export const MAX_START_TEXT = 100_000;
/** From this many helpers the picker is a select, not a list of radios. */
const RADIOS_UP_TO = 4;

type Props = {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  parent: string;
  /** What the agent's live card offers now; `null` once it offers nothing. */
  offer: ChildSessionsOffer | null;
  /** The tier chip's current pick, sent as `tier`; the dialog has no control of its own. */
  tier: Tier | undefined;
  /** What a retry of a start that failed brings back. */
  initial?: StartDraft | undefined;
  /** The dialog opened: read the card again (the helpers may have changed). */
  onOpened: () => void;
  /** The card does not list the helper any more (`unknown_helper`): read it again. */
  onCardChanged: () => void;
  /** The card no longer offers the extension (`no_child_sessions`): "New subagent" goes until it does again. */
  onWithdrawn: () => void;
  /** The start was accepted (202): the draft is the caller's until the outcome. */
  onStarted: (request: string, draft: StartDraft) => void;
  /** The element that opened the dialog: the focus goes back to it. */
  returnTo: { current: HTMLElement | null };
};

/**
 * "New subagent" (child-sessions/v1, `startChildSession`): the helper from the live card, what the
 * subagent is for in one line of at most 80 characters (counted in code points, the server's rule), what
 * it is asked, and whether it closes when its round is done (on by default; the request says it as
 * `keepOpen`, the other way round). A start is asynchronous: on 202 the dialog closes and the person's
 * draft stays with the caller until the outcome; a refusal is shown in place and the draft stays here.
 * The agent's words (a helper's name and description) are text.
 */
export function NewSubagentDialog(props: Props) {
  const { open, onOpenChange, returnTo } = props;
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        onCloseAutoFocus={(event) => {
          const target = returnTo.current;
          if (target?.isConnected) {
            event.preventDefault();
            target.focus();
          }
        }}
      >
        <Form {...props} />
      </DialogContent>
    </Dialog>
  );
}

type Errors = { top?: string; helper?: string; description?: string; text?: string };

function Form({
  onOpenChange,
  parent,
  offer,
  tier,
  initial,
  onOpened,
  onCardChanged,
  onWithdrawn,
  onStarted,
}: Props) {
  const ids = { helper: useId(), description: useId(), text: useId(), counter: useId() };
  // one request id for the dialog, and for every try in it: a repeat of a refused start is the same request
  const [request] = useState(() => uuidv7());
  const helpers = offer?.agents ?? [];
  const [agent, setAgent] = useState(() =>
    initial && helpers.some((h) => h.name === initial.agent) ? initial.agent : "",
  );
  const [description, setDescription] = useState(initial?.description ?? "");
  const [text, setText] = useState(initial?.text ?? "");
  const [closeWhenDone, setCloseWhenDone] = useState(initial ? !initial.keepOpen : true);
  const [errors, setErrors] = useState<Errors>({});
  const { start, busy } = useStartSession(parent);
  // until the person picks, the first helper is the default; once a pick is gone (the card dropped it) none is
  const [defaulted, setDefaulted] = useState(true);
  const pickerRef = useRef<HTMLFieldSetElement>(null);
  const descriptionRef = useRef<HTMLInputElement>(null);
  const textRef = useRef<HTMLTextAreaElement>(null);

  // the card is read again when the dialog opens: the helpers listed are the ones it lists now
  // biome-ignore lint/correctness/useExhaustiveDependencies: once, on open
  useEffect(() => onOpened(), []);
  // a helper the card dropped is no pick, and no other is chosen in its place
  const chosen = helpers.some((h) => h.name === agent)
    ? agent
    : agent === "" && defaulted
      ? (helpers[0]?.name ?? "")
      : "";
  const helper = helpers.find((h) => h.name === chosen);

  const verdict = checkStartDescription(description);
  const counted = "length" in verdict ? verdict.length : codePointLength(description);

  const focusPicker = () =>
    pickerRef.current?.querySelector<HTMLElement>('[role="radio"], select')?.focus();

  async function submit(event: FormEvent) {
    event.preventDefault();
    if (busy || !offer) return;
    const found: Errors = {};
    if (!verdict.ok) {
      found.description =
        verdict.reason === "too_long"
          ? `The description has ${verdict.length} characters; at most ${MAX_START_DESCRIPTION}.`
          : verdict.reason === "unreadable"
            ? "The description is one line without control or invisible characters."
            : refusalWords("bad_description", undefined);
    }
    if (chosen === "") found.helper = "Pick a subagent.";
    if (text.trim() === "") found.text = "Write what the subagent should do.";
    else if (codePointLength(text) > MAX_START_TEXT) {
      found.text = `The prompt is at most ${MAX_START_TEXT.toLocaleString("en-US")} characters.`;
    }
    if (found.helper || found.description || found.text) {
      setErrors(found);
      if (found.helper) focusPicker();
      else (found.description ? descriptionRef : textRef).current?.focus();
      return;
    }
    const draft: StartDraft = {
      agent: chosen,
      description: verdict.ok ? verdict.value : description,
      text,
      keepOpen: !closeWhenDone,
    };
    const result = await start(request, draft, tier);
    if (result.ok) {
      onStarted(request, draft);
      onOpenChange(false);
      return;
    }
    // each refusal where it belongs; anything else (a limit, a lost connection) at the top
    if (result.code === "unknown_helper") {
      setAgent("");
      setDefaulted(false);
      setErrors({ helper: result.words });
      onCardChanged();
    } else if (result.code === "bad_description") {
      setErrors({ description: result.words });
      descriptionRef.current?.focus();
    } else {
      setErrors({ top: result.words });
      if (result.code === "no_child_sessions") onWithdrawn();
    }
  }

  return (
    <form onSubmit={(e) => void submit(e)} className="grid gap-4" noValidate>
      <DialogHeader>
        <DialogTitle>New subagent</DialogTitle>
        <DialogDescription>
          A subagent works on its own while you keep talking here. Its work is a thread of its own.
        </DialogDescription>
      </DialogHeader>

      {offer === null ? (
        <InlineStatus tone="warning" role="status">
          {refusalWords("no_child_sessions", undefined)}
        </InlineStatus>
      ) : null}
      {errors.top ? (
        <InlineStatus tone="error" role="alert">
          {errors.top}
        </InlineStatus>
      ) : null}

      {offer !== null ? (
        <>
          <fieldset
            ref={pickerRef}
            className="grid gap-2"
            aria-describedby={errors.helper ? ids.helper : undefined}
          >
            <legend className="mb-1 text-sm font-medium">Subagent</legend>
            {helpers.length <= RADIOS_UP_TO ? (
              <RadioGroup
                value={chosen}
                onValueChange={(v) => {
                  setAgent(v);
                  if (errors.helper) setErrors((er) => ({ ...er, helper: undefined }));
                }}
                className="gap-2"
              >
                {helpers.map((h, i) => (
                  <Label
                    key={h.name}
                    htmlFor={`${ids.helper}-${i}`}
                    className="items-start gap-2.5 rounded-lg border p-2.5 font-normal has-[[data-state=checked]]:border-foreground/40"
                  >
                    <RadioGroupItem id={`${ids.helper}-${i}`} value={h.name} className="mt-0.5" />
                    <span className="grid min-w-0 gap-0.5">
                      <span className="font-medium [overflow-wrap:anywhere]">{h.name}</span>
                      {h.description ? (
                        <span className="text-[0.8125rem] leading-snug text-muted-foreground [overflow-wrap:anywhere]">
                          {h.description}
                        </span>
                      ) : null}
                    </span>
                  </Label>
                ))}
              </RadioGroup>
            ) : (
              <>
                <NativeSelect
                  aria-label="Subagent"
                  value={chosen}
                  onChange={(e) => {
                    setAgent(e.target.value);
                    if (errors.helper) setErrors((er) => ({ ...er, helper: undefined }));
                  }}
                  className="w-full"
                >
                  {chosen === "" ? (
                    <NativeSelectOption value="" disabled>
                      Choose a subagent
                    </NativeSelectOption>
                  ) : null}
                  {helpers.map((h) => (
                    <NativeSelectOption key={h.name} value={h.name}>
                      {h.name}
                    </NativeSelectOption>
                  ))}
                </NativeSelect>
                {helper?.description ? (
                  <p className="text-[0.8125rem] text-muted-foreground [overflow-wrap:anywhere]">
                    {helper.description}
                  </p>
                ) : null}
              </>
            )}
            {errors.helper ? (
              <p id={ids.helper} role="alert" className="text-[0.8125rem] text-destructive">
                {errors.helper}
              </p>
            ) : null}
          </fieldset>

          <div className="grid gap-1.5">
            <div className="flex items-baseline justify-between gap-2">
              <Label htmlFor={ids.description}>What it is for</Label>
              <span
                id={ids.counter}
                data-testid="description-count"
                className={`text-xs tabular-nums ${counted > MAX_START_DESCRIPTION ? "text-destructive" : "text-muted-foreground"}`}
              >
                {counted}/{MAX_START_DESCRIPTION}
              </span>
            </div>
            <Input
              id={ids.description}
              ref={descriptionRef}
              value={description}
              onChange={(e) => {
                setDescription(e.target.value);
                if (errors.description) setErrors((er) => ({ ...er, description: undefined }));
              }}
              autoComplete="off"
              placeholder="Plan the release"
              aria-invalid={errors.description ? true : undefined}
              aria-describedby={[ids.counter, errors.description ? `${ids.description}-error` : ""]
                .filter(Boolean)
                .join(" ")}
            />
            {errors.description ? (
              <p
                id={`${ids.description}-error`}
                role="alert"
                className="text-[0.8125rem] text-destructive"
              >
                {errors.description}
              </p>
            ) : null}
          </div>

          <div className="grid gap-1.5">
            <Label htmlFor={ids.text}>What to ask it</Label>
            <textarea
              id={ids.text}
              ref={textRef}
              value={text}
              onChange={(e) => {
                setText(e.target.value);
                if (errors.text) setErrors((er) => ({ ...er, text: undefined }));
              }}
              rows={5}
              aria-invalid={errors.text ? true : undefined}
              aria-describedby={errors.text ? `${ids.text}-error` : undefined}
              className="min-h-24 w-full resize-y rounded-md border border-input bg-transparent px-2.5 py-2 text-base shadow-xs outline-none placeholder:text-muted-foreground focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50 aria-invalid:border-destructive aria-invalid:ring-3 aria-invalid:ring-destructive/20 md:text-sm dark:bg-input/30"
            />
            {errors.text ? (
              <p
                id={`${ids.text}-error`}
                role="alert"
                className="text-[0.8125rem] text-destructive"
              >
                {errors.text}
              </p>
            ) : null}
          </div>

          <div className="flex items-start gap-2.5">
            <input
              id={`${ids.text}-close`}
              type="checkbox"
              checked={closeWhenDone}
              onChange={(e) => setCloseWhenDone(e.target.checked)}
              className="mt-0.5 size-4 shrink-0 accent-foreground"
            />
            <Label htmlFor={`${ids.text}-close`} className="grid gap-0.5 font-normal">
              <span className="font-medium">Close when done</span>
              <span className="text-[0.8125rem] leading-snug text-muted-foreground">
                Leave it off to keep the subagent open for more messages after its first round.
              </span>
            </Label>
          </div>
        </>
      ) : null}

      <DialogFooter>
        <Button type="button" variant="outline" onClick={() => onOpenChange(false)}>
          Cancel
        </Button>
        <Button type="submit" disabled={busy || offer === null}>
          {busy ? "Starting…" : "Start subagent"}
        </Button>
      </DialogFooter>
    </form>
  );
}
