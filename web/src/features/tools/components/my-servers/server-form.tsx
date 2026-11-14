"use client";

import { type FormEvent, useEffect, useId, useState } from "react";
import { InlineStatus } from "@/components/inline-status";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { NativeSelect, NativeSelectOption } from "@/components/ui/native-select";
import type { ApiUserToolServer } from "@/lib/api/types";
import {
  KIND_HELP,
  type Kind,
  kindLabel,
  moveWarning,
  type ServerFields,
  takesSecret,
} from "../../lib/user-servers";

/** `add`: a new server. `edit`: its name, address and authentication. `key`: a new token or key and nothing else. */
export type FormMode = "add" | "edit" | "key";

/** The field of the form a refusal is about, where its `code` names one (`url_refused`: the address). */
export type FormField = "url" | "kind";

export type FormOutcome = { ok: true } | { ok: false; message: string; field?: FormField };

type Props = {
  mode: FormMode;
  /** The kinds the deployment accepts (`UserToolServers.kinds`): the select offers these and no others. */
  kinds: readonly Kind[];
  /** The row being edited, for `edit` and `key`. */
  server?: ApiUserToolServer;
  /** Sends what was typed. The form has already cleared its secret when this is called. */
  onSubmit: (fields: ServerFields) => Promise<FormOutcome>;
  onCancel?: () => void;
};

const SECRET_LABEL: Record<string, string> = { bearer: "Token", api_key: "API key" };

const start = (kinds: readonly Kind[], server?: ApiUserToolServer): ServerFields => ({
  name: server?.name ?? "",
  url: server?.url ?? "",
  kind: server?.auth.kind ?? kinds[0] ?? "none",
  header: server?.auth.header ?? "",
  scopes: server?.auth.scopes?.join(" ") ?? "",
  clientId: server?.auth.clientId ?? "",
  secret: "",
});

/**
 * The form of a server of the person's own (ADR 0046): the same fields to add one, to change one and, with only the
 * secret, to give it a new key. The kinds come from the list's `kinds` and each says what it asks for. **The secret is
 * write-only**: it is typed into a password field that no answer ever fills in, it is cleared the moment it is sent
 * (a refusal means typing it again) and it is kept nowhere else (not in storage, not in the address).
 */
export function ServerForm({ mode, kinds, server, onSubmit, onCancel }: Props) {
  const id = useId();
  const [fields, setFields] = useState<ServerFields>(() => start(kinds, server));
  const [pending, setPending] = useState(false);
  const [problem, setProblem] = useState<{ message: string; field?: FormField } | null>(null);
  const set = <K extends keyof ServerFields>(key: K, value: ServerFields[K]) =>
    setFields((f) => ({ ...f, [key]: value }));

  // the kind the server has stays selectable even when the deployment no longer lists it, so it is shown as it is
  const offered: Kind[] =
    server && !kinds.includes(server.auth.kind) ? [server.auth.kind, ...kinds] : [...kinds];
  const keyOnly = mode === "key";
  const wantsSecret = takesSecret(fields.kind);
  const noun = SECRET_LABEL[fields.kind] ?? "Secret";
  const secretLabel = mode === "add" ? noun : `New ${noun.toLowerCase()}`;
  const warning = server && mode === "edit" ? moveWarning(server, fields) : null;
  const complete = keyOnly
    ? fields.secret !== ""
    : fields.name.trim() !== "" &&
      fields.url.trim() !== "" &&
      (fields.kind !== "api_key" || fields.header.trim() !== "") &&
      (mode !== "add" || !wantsSecret || fields.secret !== "");

  async function submit(event: FormEvent) {
    event.preventDefault();
    if (pending || !complete) return;
    const sent = fields;
    // write-only: gone from this form the moment it is on its way
    setFields((f) => ({ ...f, secret: "" }));
    setProblem(null);
    setPending(true);
    const outcome = await onSubmit(sent);
    setPending(false);
    if (outcome.ok) {
      if (mode === "add") setFields(start(kinds));
    } else {
      setProblem({ message: outcome.message, ...(outcome.field ? { field: outcome.field } : {}) });
    }
  }

  // a refusal about a field: the words are tied to it (`aria-invalid`, `aria-describedby`) and the focus goes there
  const bad = problem?.field;
  // biome-ignore lint/correctness/useExhaustiveDependencies: a new refusal about the same field moves the focus again
  useEffect(() => {
    if (bad) document.getElementById(`${id}-${bad}`)?.focus();
  }, [bad, id, problem]);
  const errorId = `${id}-error`;
  const tie = (field: FormField, help: string) => (bad === field ? `${help} ${errorId}` : help);

  return (
    <form
      onSubmit={submit}
      noValidate
      data-slot="server-form"
      data-mode={mode}
      className="flex flex-col gap-4"
    >
      {keyOnly ? null : (
        <>
          <div className="flex flex-col gap-1.5">
            <Label htmlFor={`${id}-name`}>Name</Label>
            <Input
              id={`${id}-name`}
              value={fields.name}
              maxLength={80}
              autoComplete="off"
              onChange={(e) => set("name", e.target.value)}
            />
          </div>
          <div className="flex flex-col gap-1.5">
            <Label htmlFor={`${id}-url`}>Address</Label>
            <Input
              id={`${id}-url`}
              value={fields.url}
              inputMode="url"
              placeholder="https://mcp.example.com/mcp"
              autoComplete="off"
              autoCapitalize="none"
              spellCheck={false}
              aria-invalid={bad === "url" ? true : undefined}
              aria-describedby={tie("url", `${id}-url-help`)}
              onChange={(e) => set("url", e.target.value)}
            />
            <p id={`${id}-url-help`} className="text-xs text-muted-foreground">
              The server’s MCP address, starting with https://. Addresses on a private network are
              refused.
            </p>
          </div>
          <div className="flex flex-col gap-1.5">
            <Label htmlFor={`${id}-kind`}>Authentication</Label>
            <NativeSelect
              id={`${id}-kind`}
              value={fields.kind}
              aria-invalid={bad === "kind" ? true : undefined}
              aria-describedby={tie("kind", `${id}-kind-help`)}
              className="w-full sm:w-64"
              onChange={(e) => set("kind", e.target.value as Kind)}
            >
              {offered.map((kind) => (
                <NativeSelectOption key={kind} value={kind}>
                  {kindLabel(kind)}
                </NativeSelectOption>
              ))}
            </NativeSelect>
            <p id={`${id}-kind-help`} className="text-xs text-muted-foreground">
              {KIND_HELP[fields.kind]}
            </p>
          </div>
          {fields.kind === "api_key" ? (
            <div className="flex flex-col gap-1.5">
              <Label htmlFor={`${id}-header`}>Header name</Label>
              <Input
                id={`${id}-header`}
                value={fields.header}
                placeholder="X-Api-Key"
                autoComplete="off"
                autoCapitalize="none"
                spellCheck={false}
                onChange={(e) => set("header", e.target.value)}
              />
            </div>
          ) : null}
          {fields.kind === "oauth2" ? (
            <>
              <div className="flex flex-col gap-1.5">
                <Label htmlFor={`${id}-scopes`}>Scopes (optional)</Label>
                <Input
                  id={`${id}-scopes`}
                  value={fields.scopes}
                  placeholder="read write"
                  autoComplete="off"
                  autoCapitalize="none"
                  spellCheck={false}
                  onChange={(e) => set("scopes", e.target.value)}
                />
              </div>
              <div className="flex flex-col gap-1.5">
                <Label htmlFor={`${id}-client`}>Client ID (optional)</Label>
                <Input
                  id={`${id}-client`}
                  value={fields.clientId}
                  autoComplete="off"
                  autoCapitalize="none"
                  spellCheck={false}
                  onChange={(e) => set("clientId", e.target.value)}
                />
              </div>
            </>
          ) : null}
        </>
      )}
      {wantsSecret ? (
        <div className="flex flex-col gap-1.5">
          <Label htmlFor={`${id}-secret`}>{secretLabel}</Label>
          <Input
            id={`${id}-secret`}
            data-slot="secret"
            type="password"
            value={fields.secret}
            autoComplete="new-password"
            autoCapitalize="none"
            spellCheck={false}
            aria-describedby={`${id}-secret-help`}
            onChange={(e) => set("secret", e.target.value)}
          />
          <p id={`${id}-secret-help`} className="text-xs text-muted-foreground">
            {mode === "add"
              ? "Kept sealed on the server and never shown again."
              : "Leave it empty to keep the current one. A new one is kept sealed and never shown again."}
          </p>
        </div>
      ) : null}
      {warning ? (
        <InlineStatus tone="warning" role="status">
          {warning}
        </InlineStatus>
      ) : null}
      {problem ? (
        <div id={errorId}>
          <InlineStatus tone="error" role="alert">
            {problem.message}
          </InlineStatus>
        </div>
      ) : null}
      <div className="flex flex-wrap items-center gap-2">
        <Button type="submit" disabled={pending || !complete}>
          {pending
            ? "Saving…"
            : mode === "add"
              ? "Add server"
              : keyOnly
                ? "Save key"
                : "Save changes"}
        </Button>
        {onCancel ? (
          <Button type="button" variant="ghost" onClick={onCancel} disabled={pending}>
            Cancel
          </Button>
        ) : null}
      </div>
    </form>
  );
}
