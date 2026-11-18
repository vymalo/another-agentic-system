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
import type { ApiPrompt, ApiPromptSummary, ApiUpdatePrompt } from "@/lib/api/types";
import type { WriteResult } from "../hooks/use-prompt-library";
import type { Failure } from "../lib/failures";
import type { PromptRead } from "../lib/read";
import {
  audienceField,
  audienceFits,
  audienceIn,
  audienceSentence,
  buildShare,
  shareNotice,
} from "../lib/share";

/** What the page says after the dialog closed: a share went through, or the prompt is gone. */
export type ShareNotice = { message: string; tone: "muted" | "warning"; gone?: boolean };

type Props = {
  /** The row as the page read it: its title, `/name`, audience and the `revision` the dialog opens at. */
  prompt: ApiPromptSummary;
  /** `updatePrompt` for this prompt: `{revision, audience}`, and nothing else. */
  write: (body: ApiUpdatePrompt) => Promise<WriteResult<ApiPrompt>>;
  /** `getPrompt` again, for "Load the current version". */
  read: () => Promise<PromptRead>;
  onClose: (notice?: ShareNotice) => void;
};

const joined = (...ids: (string | false | null | undefined)[]): string | undefined => {
  const out = ids.filter(Boolean).join(" ");
  return out === "" ? undefined : out;
};

/**
 * Who may read a prompt (docs/api/prompts-v1.md, "The audience"): a comma-separated field of role values, `*` alone for
 * everyone and empty for nobody but the owner. **The server judges every value**: nothing here refuses a role, and a
 * `bad_audience` is said under the field, with `aria-invalid` and the focus. A stale share keeps the roles typed and
 * offers the current audience; a 403 offers **Stop sharing**, the only write a person without `prompts.share` may make.
 * A role the field cannot show (one with a comma) is kept as it was while the field is untouched, and said there.
 */
export function SharePromptDialog({ prompt, write, read, onClose }: Props) {
  const id = useId();
  const fieldRef = useRef<HTMLInputElement>(null);
  const alive = useRef(true);
  const [field, setField] = useState(() => audienceField(prompt.audience));
  // the audience the prompt has, as the server last said it: what an untouched field means (`audienceIn`)
  const [stored, setStored] = useState<readonly string[]>(() => prompt.audience ?? []);
  const [revision, setRevision] = useState(prompt.revision);
  const [acting, setActing] = useState<"save" | "stop" | null>(null);
  const [reading, setReading] = useState(false);
  const [failure, setFailure] = useState<Failure | null>(null);
  const [readError, setReadError] = useState<string | null>(null);

  // (StrictMode runs the cleanup and the effect again: the ref is set true in the body, not only at mount)
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);

  // a refusal of the audience moves the focus to the field (a new refusal about it moves it again)
  useEffect(() => {
    if (failure?.target.field === "audience") fieldRef.current?.focus();
  }, [failure]);

  const busy = acting !== null || reading;
  const values = audienceIn(field, stored);
  // a role the field cannot show (one with a comma): said, and kept while the field is untouched
  const untouched = field === audienceField(stored);
  const comma = !audienceFits(stored) && untouched;
  const fieldError = failure?.target.field === "audience" ? failure.message : null;
  // every other refusal is the dialog's own: a stale share, a lost permission, a prompt that is gone, a failed request
  const topFailure = failure && failure.target.field !== "audience" ? failure : null;
  const errorId = `${id}-error`;

  /** A refused write: a prompt that is gone closes the dialog, and anything else is said where it is about. */
  function refused(failure: Failure) {
    if (failure.gone) {
      onClose({ message: failure.message, tone: "warning", gone: true });
      return;
    }
    setFailure(failure);
  }

  async function save(event: FormEvent) {
    event.preventDefault();
    if (busy) return;
    setFailure(null);
    setReadError(null);
    setActing("save");
    const audience = audienceIn(field, stored);
    const result = await write(buildShare(revision, audience));
    if (!alive.current) return;
    setActing(null);
    if (result.ok) {
      onClose({
        message: shareNotice(prompt.title, result.value.audience ?? audience),
        tone: "muted",
      });
      return;
    }
    refused(result.failure);
  }

  async function stop() {
    if (busy) return;
    setFailure(null);
    setReadError(null);
    setActing("stop");
    const result = await write(buildShare(revision, []));
    if (!alive.current) return;
    setActing(null);
    if (result.ok) {
      onClose({ message: `“${prompt.title}” is no longer shared.`, tone: "muted" });
      return;
    }
    refused(result.failure);
  }

  /** "Load the current version": the prompt is read again, and the field shows who it is shared with now. */
  async function loadCurrent() {
    if (busy) return;
    setReading(true);
    setReadError(null);
    const result = await read();
    if (!alive.current) return;
    setReading(false);
    if (!result.ok) {
      if (result.failure.gone) {
        onClose({ message: result.failure.message, tone: "warning", gone: true });
        return;
      }
      setReadError(result.failure.message);
      return;
    }
    setRevision(result.value.revision);
    setStored(result.value.audience ?? []);
    setField(audienceField(result.value.audience));
    setFailure(null);
    // the InlineStatus that held the focus is gone: the field the person is about to Save takes it
    requestAnimationFrame(() => fieldRef.current?.focus());
  }

  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open && !busy) onClose();
      }}
    >
      <DialogContent
        data-slot="share-prompt-dialog"
        onOpenAutoFocus={(event) => {
          event.preventDefault();
          fieldRef.current?.focus();
        }}
      >
        <DialogHeader>
          <DialogTitle className="break-words">Share “{prompt.title}”</DialogTitle>
          <DialogDescription>
            <span className="font-mono break-all">/{prompt.name}</span>
          </DialogDescription>
        </DialogHeader>

        <form onSubmit={save} noValidate className="flex flex-col gap-4">
          <div className="flex flex-col gap-1.5">
            <Label htmlFor={`${id}-audience`}>Roles who can read it</Label>
            <Input
              id={`${id}-audience`}
              ref={fieldRef}
              value={field}
              autoComplete="off"
              autoCapitalize="none"
              spellCheck={false}
              disabled={busy}
              aria-invalid={fieldError ? true : undefined}
              aria-describedby={joined(
                `${id}-help`,
                `${id}-live`,
                comma && `${id}-comma`,
                fieldError && errorId,
              )}
              onChange={(event) => setField(event.target.value)}
            />
            <p id={`${id}-help`} className="text-xs text-muted-foreground">
              The roles your deployment gives, separated by commas. You can name a role you do not
              hold. Type * alone for everyone; leave it empty so that only you can read it.
            </p>
            <p id={`${id}-live`} className="text-sm text-muted-foreground">
              {audienceSentence(values)}
            </p>
            {comma ? (
              <p id={`${id}-comma`} className="text-xs text-muted-foreground">
                One of these roles holds a comma, which this field uses between roles. The stored
                role is kept while the field reads exactly as it does now; any other text is split
                on the commas as typed.
              </p>
            ) : null}
            {fieldError ? (
              <p id={errorId} role="alert" className="text-sm text-destructive">
                {fieldError}
              </p>
            ) : null}
          </div>

          {topFailure ? (
            <InlineStatus
              tone={topFailure.stale ? "warning" : "error"}
              role="alert"
              {...(topFailure.stale
                ? {
                    action: {
                      label: "Load the current version",
                      onClick: () => void loadCurrent(),
                    },
                  }
                : {})}
            >
              {topFailure.message}
            </InlineStatus>
          ) : null}
          {readError ? (
            <InlineStatus tone="error" role="alert">
              {readError}
            </InlineStatus>
          ) : null}

          <DialogFooter>
            <Button type="button" variant="ghost" onClick={() => onClose()} disabled={busy}>
              Cancel
            </Button>
            {topFailure?.forbidden ? (
              <Button type="button" variant="outline" onClick={() => void stop()} disabled={busy}>
                {acting === "stop" ? "Stopping…" : "Stop sharing"}
              </Button>
            ) : null}
            <Button type="submit" disabled={busy}>
              {acting === "save" ? "Saving…" : "Save"}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
