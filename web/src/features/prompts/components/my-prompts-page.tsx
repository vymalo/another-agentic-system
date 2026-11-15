"use client";

import { ArrowLeftIcon, PlusIcon } from "lucide-react";
import Link from "next/link";
import { useEffect, useRef, useState } from "react";
import { InlineStatus, LoadingStatus } from "@/components/inline-status";
import { Button } from "@/components/ui/button";
import { useUiConfig } from "@/features/chat/hooks/use-ui-config";
import { PROMPTS_LABEL } from "@/features/session/components/account-menu";
import { openSignIn } from "@/lib/api/session";
import type { ApiPrompt, ApiPromptSummary } from "@/lib/api/types";
import { usePromptLibrary } from "../hooks/use-prompt-library";
import {
  buildChange,
  buildCreate,
  fieldsOf,
  type PromptFields,
  questionIds,
  splitPrompts,
} from "../lib/library";
import { DeletePromptDialog } from "./delete-prompt-dialog";
import { type FormOutcome, PromptForm } from "./prompt-form";
import { PromptRow } from "./prompt-row";
import { SharedPromptView } from "./shared-prompt-view";

type Limits = {
  limit: number;
  maxVariables: number;
  maxBodyBytes: number;
  maxRenderedBytes: number;
};

const BLANK: PromptFields = { title: "", name: "", description: "", body: "", questions: [] };

/**
 * `/prompts`: the person's prompt library (ADR 0070): list, create, edit, rename, delete, read what others shared and copy it.
 * Nothing is asked until the configuration says the deployment has a library (`prompts` of `GET /api/config`). Owning prompts
 * needs no permission, so nothing here is hidden by one. The server decides every value; this page draws its answers.
 */
export function MyPromptsPage() {
  const ui = useUiConfig();
  const prompts = ui.config.prompts;
  return (
    <main className="mx-auto flex min-h-dvh w-full max-w-3xl flex-col gap-6 px-4 py-6 md:px-6">
      <header className="flex flex-col gap-3">
        <Button asChild variant="ghost" size="sm" className="-ms-2 self-start">
          <Link href="/">
            <ArrowLeftIcon aria-hidden="true" />
            Back to chat
          </Link>
        </Button>
        <h1 className="text-[1.75rem] leading-tight font-medium tracking-tight">{PROMPTS_LABEL}</h1>
        <p className="text-sm text-muted-foreground">
          Prompts you save here are yours: a text with blanks, and a question for each blank. You
          can also read the prompts others share with you, and copy them into yours.
        </p>
      </header>
      {!ui.loaded ? <LoadingStatus>Loading your prompts</LoadingStatus> : null}
      {ui.loaded && !prompts ? (
        <InlineStatus role="status">This deployment does not offer a prompt library.</InlineStatus>
      ) : null}
      {ui.loaded && prompts ? <Library config={prompts} /> : null}
    </main>
  );
}

type View =
  | { kind: "list" }
  | { kind: "opening" }
  | {
      kind: "form";
      mode: "create" | "edit" | "copy";
      heading: string;
      initial: PromptFields;
      /** Edit: the prompt as it was read, which is what a save is compared with and carries the revision of. */
      base?: ApiPrompt;
      /** A new token remounts the form, so "Load the current version" starts it again from what was read. */
      token: number;
    }
  | { kind: "shared"; prompt: ApiPrompt };

function Library({ config }: { config: Limits }) {
  const lib = usePromptLibrary(config);
  const { state } = lib;
  const [view, setView] = useState<View>({ kind: "list" });
  const [notice, setNotice] = useState<{ text: string; tone: "muted" | "warning" } | null>(null);
  const [deleting, setDeleting] = useState<ApiPromptSummary | null>(null);
  const opener = useRef<string | null>(null);
  const tokens = useRef(0);
  // the opening that is current: an answer for an earlier one is ignored
  const opening = useRef(0);

  // when the list is back, the focus returns to the button that opened what was closed (or the list, if that is gone)
  const listing = view.kind === "list";
  useEffect(() => {
    if (!listing || state.status !== "ready") return;
    const key = opener.current;
    opener.current = null;
    if (!key) return;
    const target =
      document.querySelector<HTMLElement>(`[data-opener="${key}"]`) ??
      document.getElementById("my-prompts-list");
    target?.focus();
  }, [listing, state.status]);

  const close = (text?: string) => {
    opening.current++;
    setNotice(text ? { text, tone: "muted" } : null);
    setView({ kind: "list" });
  };

  /** A prompt that could not be opened: said, the list read again where it is gone. */
  const refused = (message: string, reread: boolean) => {
    setNotice({ text: message, tone: "warning" });
    setView({ kind: "list" });
    if (reread) lib.reload();
  };

  async function open(summary: ApiPromptSummary, key: string, then: (p: ApiPrompt) => View) {
    opener.current = key;
    const mine = ++opening.current;
    setNotice(null);
    setView({ kind: "opening" });
    const result = await lib.get(summary.id);
    if (mine !== opening.current) return;
    if (result.ok) setView(then(result.value));
    else refused(result.failure.message, result.failure.reread);
  }

  const edit = (summary: ApiPromptSummary) =>
    open(summary, `edit:${summary.id}`, (p) => ({
      kind: "form",
      mode: "edit",
      heading: `Edit “${p.title}”`,
      initial: fieldsOf(p),
      base: p,
      token: ++tokens.current,
    }));
  const read = (summary: ApiPromptSummary) =>
    open(summary, `view:${summary.id}`, (p) => ({ kind: "shared", prompt: p }));
  const copy = (summary: ApiPromptSummary) =>
    open(summary, `copy:${summary.id}`, (p) => copyView(p));

  const copyView = (p: ApiPrompt): View => ({
    kind: "form",
    mode: "copy",
    heading: "Copy to my prompts",
    // what was read, and no audience: a copy is private, and is the person's own to change
    initial: fieldsOf(p),
    token: ++tokens.current,
  });

  function create() {
    opener.current = "new";
    opening.current++;
    setNotice(null);
    setView({
      kind: "form",
      mode: "create",
      heading: "New prompt",
      initial: BLANK,
      token: ++tokens.current,
    });
  }

  async function save(fields: PromptFields): Promise<FormOutcome> {
    if (view.kind !== "form") return { ok: true };
    const shown = {
      name: fields.name.trim(),
      body: fields.body,
      ids: questionIds(fields.questions),
    };
    if (view.mode === "edit" && view.base) {
      const change = buildChange(view.base, fields);
      if (change === null) {
        close();
        return { ok: true };
      }
      const result = await lib.update(view.base.id, change, shown);
      if (!result.ok) return result;
      close(`Saved “${result.value.title}”.`);
      return { ok: true };
    }
    const result = await lib.create(buildCreate(fields), shown);
    if (!result.ok) return result;
    close(`Saved “${result.value.title}”.`);
    return { ok: true };
  }

  /** "Load the current version": the prompt is read again and the form starts from it. Answers why it could not. */
  async function loadCurrent(): Promise<string | null> {
    if (view.kind !== "form" || !view.base) return null;
    const result = await lib.get(view.base.id);
    if (!result.ok) {
      if (result.failure.gone) refused(result.failure.message, true);
      return result.failure.message;
    }
    setView({
      kind: "form",
      mode: "edit",
      heading: `Edit “${result.value.title}”`,
      initial: fieldsOf(result.value),
      base: result.value,
      token: ++tokens.current,
    });
    return null;
  }

  return (
    <>
      {state.status === "loading" ? <LoadingStatus>Loading your prompts</LoadingStatus> : null}
      {state.status === "off" ? (
        <InlineStatus role="status">This deployment does not offer a prompt library.</InlineStatus>
      ) : null}
      {state.status === "no-access" ? (
        <InlineStatus role="status">
          Your roles grant no access to this app. Ask whoever runs this deployment for a role.
        </InlineStatus>
      ) : null}
      {state.status === "signed-out" ? (
        <InlineStatus
          role="status"
          action={{
            label: "Sign in",
            // a deployment with no sign-in built in has nothing to open: reading again is all there is
            onClick: () => {
              if (openSignIn() === "none") lib.reload();
            },
          }}
        >
          You are signed out. Sign in to see your prompts.
        </InlineStatus>
      ) : null}
      {state.status === "error" ? (
        <InlineStatus
          tone="error"
          role="alert"
          action={{ label: "Try again", onClick: lib.reload }}
        >
          Your prompts could not be loaded. Try again in a moment.
        </InlineStatus>
      ) : null}
      {state.status === "ready" ? (
        <>
          {state.readError ? (
            <InlineStatus
              tone="warning"
              role="status"
              action={{ label: "Try again", onClick: lib.reload }}
            >
              The list could not be refreshed. What is shown may be out of date.
            </InlineStatus>
          ) : null}
          {notice ? (
            <InlineStatus tone={notice.tone} role="status">
              {notice.text}
            </InlineStatus>
          ) : null}
          {view.kind === "opening" ? <LoadingStatus>Loading the prompt</LoadingStatus> : null}
          {view.kind === "form" ? (
            <div className="rounded-xl border bg-card px-4 py-4">
              <PromptForm
                key={view.token}
                heading={view.heading}
                mode={view.mode}
                initial={view.initial}
                limits={config}
                onSubmit={save}
                onCancel={() => close()}
                {...(view.mode === "edit" ? { onLoadCurrent: loadCurrent } : {})}
              />
            </div>
          ) : null}
          {view.kind === "shared" ? (
            <SharedPromptView
              prompt={view.prompt}
              canCopy={ownCount(state.prompts) < config.limit}
              onBack={() => close()}
              onCopy={() => {
                opener.current = `view:${view.prompt.id}`;
                setView(copyView(view.prompt));
              }}
            />
          ) : null}
          {view.kind === "list" ? (
            <ListArea
              prompts={state.prompts}
              limit={config.limit}
              onCreate={create}
              onEdit={edit}
              onDelete={setDeleting}
              onView={read}
              onCopy={copy}
            />
          ) : null}
          {deleting ? (
            <DeletePromptDialog
              prompt={deleting}
              onDelete={lib.remove}
              onClose={() => setDeleting(null)}
            />
          ) : null}
        </>
      ) : null}
    </>
  );
}

const ownCount = (list: readonly ApiPromptSummary[]) => list.filter((p) => p.own).length;

type ListProps = {
  prompts: ApiPromptSummary[];
  limit: number;
  onCreate: () => void;
  onEdit: (p: ApiPromptSummary) => void;
  onDelete: (p: ApiPromptSummary) => void;
  onView: (p: ApiPromptSummary) => void;
  onCopy: (p: ApiPromptSummary) => void;
};

function ListArea({ prompts, limit, onCreate, onEdit, onDelete, onView, onCopy }: ListProps) {
  const { own, shared } = splitPrompts(prompts);
  const atLimit = own.length >= limit;
  return (
    <>
      <section aria-labelledby="my-prompts-list" className="flex flex-col gap-3">
        <h2 id="my-prompts-list" tabIndex={-1} className="text-sm font-medium outline-none">
          Your prompts{" "}
          <span className="font-normal text-muted-foreground">
            ({own.length} of {limit})
          </span>
        </h2>
        {own.length === 0 ? (
          <p className="text-sm text-muted-foreground">You have no prompts yet.</p>
        ) : (
          <ul aria-label="Your prompts" className="flex flex-col gap-3">
            {own.map((p) => (
              <PromptRow
                key={p.id}
                prompt={p}
                onEdit={onEdit}
                onDelete={onDelete}
                onView={onView}
                onCopy={onCopy}
              />
            ))}
          </ul>
        )}
        {atLimit ? (
          <p className="text-sm text-muted-foreground">
            You have {limit} prompts, the most one person can have. Delete one to add another.
          </p>
        ) : (
          <Button
            type="button"
            variant="outline"
            className="self-start"
            data-opener="new"
            onClick={onCreate}
          >
            <PlusIcon aria-hidden="true" />
            New prompt
          </Button>
        )}
      </section>
      {shared.length > 0 ? (
        <section aria-labelledby="shared-prompts-list" className="flex flex-col gap-3">
          <h2 id="shared-prompts-list" className="text-sm font-medium">
            Shared with you{" "}
            <span className="font-normal text-muted-foreground">({shared.length})</span>
          </h2>
          <ul aria-label="Shared with you" className="flex flex-col gap-3">
            {shared.map((p) => (
              <PromptRow
                key={p.id}
                prompt={p}
                onEdit={onEdit}
                onDelete={onDelete}
                onView={onView}
                onCopy={onCopy}
              />
            ))}
          </ul>
        </section>
      ) : null}
    </>
  );
}
