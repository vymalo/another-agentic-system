"use client";

import { XIcon } from "lucide-react";
import { type FormEvent, useId, useState } from "react";
import { InlineStatus } from "@/components/inline-status";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { NativeSelect, NativeSelectOption } from "@/components/ui/native-select";
import { type SurfaceHost, useSurfaceHost } from "@/features/chat/components/surface/surface-host";
import type { TrustedHints } from "@/features/chat/lib/agui/vymalo";
import { useMe } from "@/features/me/hooks/use-me";
import { holds } from "@/features/me/lib/access";
import { useUserToolServers } from "@/features/tools/hooks/use-user-tool-servers";
import {
  buildCreate,
  hostOf,
  type Kind,
  kindLabel,
  type ServerFields,
  takesSecret,
} from "@/features/tools/lib/user-servers";
import { type ApiActor, type ApiUserToolServer, isTerminal } from "@/lib/api/types";
import { keyTakers, type Receipt, receiptOf, rekeyBody } from "../lib/receipt";
import { TrustedFallback, TrustedNote } from "./chrome";

/*
 * `McpCredential` (ADR 0083, ADR 0046): the client's own form for a person's own MCP server credential, drawn by the
 * client and by nothing the agent can write. The person types the key here; **this component** calls
 * `POST /api/user-tool-servers` (a new server) or `PATCH /api/user-tool-servers/{serverId}` (a new key) with the
 * person's own credentials, and the agent receives a **receipt** — never a value.
 *
 * The secret lives in this component's React state and nowhere else: not in a surface's data model (there is none), not
 * in a `$field` marker (`rewriteContext`) and not in an action `context` (`resolveFields`). It is an argument of one
 * call, cleared from the field the moment it is on its way, whatever the answer. `lib/receipt.ts` builds the receipt
 * from what the API answered, and its `last4` is omitted because no route returns a suffix.
 */

/** The select's value for "a server the person does not have yet". */
const NEW = "new";

/** Why the form cannot be used here, in the client's words; null when it can. */
function cannotSend(host: SurfaceHost): string | null {
  if (host.readOnly) return host.readOnly;
  if (host.state === undefined) return "The chat is not open yet.";
  if (isTerminal(host.state)) return "This request is finished: write a message to go on.";
  if (!host.canSend) return "The form works while this chat waits for you.";
  return null;
}

export function McpCredentialCard({
  card,
  hints,
  actor,
}: {
  /** The activity's `messageId` (`trusted-<seq>`): the card the receipt's action answers. */
  card: string;
  hints: TrustedHints;
  actor: ApiActor | undefined;
}) {
  const id = useId();
  const host = useSurfaceHost();
  const view = useUserToolServers();
  const me = useMe();
  const person = me.status === "ready" ? me.me : null;
  // `me` unknown: nothing is hidden and the server says no in words where the action is (as `/tools` does)
  const mayChange = person === null || holds(person, "tools.user.add");
  const who = actor?.name ?? "the agent";
  const { state } = view;

  const [target, setTarget] = useState(NEW);
  const [name, setName] = useState(hints.name ?? "");
  const [url, setUrl] = useState("");
  const [kind, setKind] = useState<Kind>("bearer");
  const [header, setHeader] = useState("");
  const [scopes, setScopes] = useState<string[]>(hints.scopes ?? []);
  const [secret, setSecret] = useState("");
  const [pending, setPending] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const [receipt, setReceipt] = useState<Receipt | null>(null);
  const [told, setTold] = useState(false);

  /** Sends the receipt as the action's `context`: the only thing that reaches the agent (ADR 0083, decision 3). */
  function tell(done: Receipt) {
    if (!host.canSend) return;
    host.send({
      name: "submit",
      // the card the person answered: the activity's `messageId` (`trusted-<seq>`)
      surfaceId: card,
      sourceComponentId: "McpCredential",
      context: { ...done },
    });
    setTold(true);
  }

  if (!mayChange) {
    return (
      <TrustedFallback>
        <p>
          Your roles do not let you add or change tool servers of your own, so this key cannot be
          saved here.
        </p>
      </TrustedFallback>
    );
  }
  if (state.status === "off") {
    return (
      <TrustedFallback>
        <p>This deployment does not let people add tool servers of their own.</p>
      </TrustedFallback>
    );
  }
  if (state.status === "no-access") {
    return (
      <TrustedFallback>
        <p>Your roles grant no access to this app, so nothing can be saved here.</p>
      </TrustedFallback>
    );
  }
  if (state.status === "signed-out") {
    return (
      <TrustedFallback>
        <p>You are signed out, so a key cannot be saved here.</p>
      </TrustedFallback>
    );
  }
  if (state.status === "error") {
    return (
      <TrustedFallback>
        <p>Your tool servers could not be read: {state.message}</p>
      </TrustedFallback>
    );
  }
  if (state.status === "loading") {
    return <InlineStatus role="status">Reading your tool servers</InlineStatus>;
  }

  const servers = state.servers;
  const takers = keyTakers(servers);
  const was: ApiUserToolServer | undefined =
    target === NEW ? undefined : servers.find((server) => server.id === target);
  // the kinds this deployment accepts that a key can be given to; none: it keeps no credentials
  const offered = state.kinds.filter(takesSecret);
  const authKind: Kind | undefined = offered.includes(kind) ? kind : offered[0];
  const keyOnly = target !== NEW;
  const complete = keyOnly
    ? secret !== ""
    : name.trim() !== "" &&
      url.trim() !== "" &&
      (authKind !== "api_key" || header.trim() !== "") &&
      secret !== "";
  const why = cannotSend(host);
  const noun = authKind === "api_key" ? "API key" : "Token";

  async function submit(event: FormEvent) {
    event.preventDefault();
    if (pending || !complete || authKind === undefined) return;
    const sent = secret;
    // write-only: gone from this card the moment it is on its way, whatever the answer
    setSecret("");
    setProblem(null);
    setPending(true);
    const fields: ServerFields = {
      name,
      url,
      kind: authKind,
      header,
      scopes: "",
      clientId: "",
      secret: sent,
    };
    const result =
      target === NEW
        ? await view.add(buildCreate(fields))
        : was
          ? await view.update(was.id, rekeyBody(was, sent))
          : null;
    setPending(false);
    if (result === null) {
      setProblem("That server is no longer in your list. Read it again on My tool servers.");
      return;
    }
    if (!result.ok) {
      // the problem's words, by its status and `code` (a 400's `detail` never quotes a value)
      setProblem(result.failure.message);
      return;
    }
    const done = receiptOf(target === NEW ? "created" : "updated", result.value, scopes);
    setReceipt(done);
    tell(done);
  }

  if (receipt) {
    return (
      <div
        data-slot="trusted-receipt"
        className="flex flex-col gap-2 text-[0.8125rem] [overflow-wrap:anywhere]"
      >
        <p>
          Saved: <strong>{receipt.name}</strong>{" "}
          <span className="text-muted-foreground">({receipt.id})</span>.
        </p>
        <TrustedNote>
          The agent is told the name, the id and what the key is for. It is never told the key.
        </TrustedNote>
        {told ? (
          <p className="text-muted-foreground">The receipt was sent.</p>
        ) : (
          <div className="flex flex-col gap-1.5">
            <Button
              type="button"
              size="sm"
              className="self-start"
              disabled={!host.canSend}
              onClick={() => tell(receipt)}
            >
              Tell the agent
            </Button>
            {host.canSend ? null : (
              <TrustedNote>The agent is told while this chat waits for you.</TrustedNote>
            )}
          </div>
        )}
      </div>
    );
  }

  if (offered.length === 0) {
    return (
      <TrustedFallback>
        <p>
          This deployment keeps no credentials, so it cannot keep a key for a tool server of your
          own.
        </p>
      </TrustedFallback>
    );
  }

  return (
    <form
      onSubmit={submit}
      noValidate
      data-slot="mcp-credential"
      className="flex flex-col gap-3"
      aria-label={`Add a key for a tool server, suggested by ${who}`}
    >
      {takers.length > 0 ? (
        <div className="flex flex-col gap-1.5">
          <Label htmlFor={`${id}-target`}>Server</Label>
          <NativeSelect
            id={`${id}-target`}
            value={target}
            className="w-full sm:w-64"
            onChange={(e) => setTarget(e.target.value)}
          >
            <NativeSelectOption value={NEW}>A new server</NativeSelectOption>
            {takers.map((server) => (
              <NativeSelectOption key={server.id} value={server.id}>
                {server.name}
              </NativeSelectOption>
            ))}
          </NativeSelect>
          {keyOnly && was ? (
            <TrustedNote>
              {was.name} at {hostOf(was.url)}. A new key replaces the one it has; its address and
              how it signs in stay as they are.
            </TrustedNote>
          ) : null}
        </div>
      ) : null}

      {keyOnly ? null : (
        <>
          <div className="flex flex-col gap-1.5">
            <Label htmlFor={`${id}-name`}>Name</Label>
            <Input
              id={`${id}-name`}
              value={name}
              maxLength={80}
              autoComplete="off"
              aria-describedby={hints.name !== undefined ? `${id}-name-help` : undefined}
              onChange={(e) => setName(e.target.value)}
            />
            {hints.name !== undefined ? (
              <p id={`${id}-name-help`} className="text-xs text-muted-foreground">
                Suggested by {who}. This is what your Tools menu shows.
              </p>
            ) : null}
          </div>
          <div className="flex flex-col gap-1.5">
            <Label htmlFor={`${id}-url`}>Address</Label>
            <Input
              id={`${id}-url`}
              value={url}
              inputMode="url"
              placeholder="https://mcp.example.com/mcp"
              autoComplete="off"
              autoCapitalize="none"
              spellCheck={false}
              aria-describedby={`${id}-url-help`}
              onChange={(e) => setUrl(e.target.value)}
            />
            <p id={`${id}-url-help`} className="text-xs text-muted-foreground">
              Your server’s MCP address, starting with https://. Addresses on a private network are
              refused.
            </p>
          </div>
          <div className="flex flex-col gap-1.5">
            <Label htmlFor={`${id}-kind`}>Authentication</Label>
            <NativeSelect
              id={`${id}-kind`}
              value={authKind}
              className="w-full sm:w-64"
              onChange={(e) => setKind(e.target.value as Kind)}
            >
              {offered.map((value) => (
                <NativeSelectOption key={value} value={value}>
                  {kindLabel(value)}
                </NativeSelectOption>
              ))}
            </NativeSelect>
          </div>
          {authKind === "api_key" ? (
            <div className="flex flex-col gap-1.5">
              <Label htmlFor={`${id}-header`}>Header name</Label>
              <Input
                id={`${id}-header`}
                value={header}
                placeholder="X-Api-Key"
                autoComplete="off"
                autoCapitalize="none"
                spellCheck={false}
                onChange={(e) => setHeader(e.target.value)}
              />
            </div>
          ) : null}
        </>
      )}

      {scopes.length > 0 ? (
        <div className="flex flex-col gap-1.5">
          <span className="text-sm font-medium">What the key is for</span>
          <ul aria-label="Scopes" className="flex flex-wrap gap-1.5">
            {scopes.map((scope) => (
              <li key={scope}>
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  aria-label={`Remove ${scope}`}
                  onClick={() => setScopes((all) => all.filter((s) => s !== scope))}
                >
                  {scope}
                  <XIcon aria-hidden="true" className="size-3.5" />
                </Button>
              </li>
            ))}
          </ul>
          <p className="text-xs text-muted-foreground">
            Suggested by {who}. Your server keeps no scopes of its own: what you keep here is what
            the agent is told this key is for.
          </p>
        </div>
      ) : null}

      {hints.expiresInDays !== undefined ? (
        <TrustedNote>
          The agent suggested this key last {hints.expiresInDays} days. This deployment keeps it
          until you replace or delete the server.
        </TrustedNote>
      ) : null}

      <div className="flex flex-col gap-1.5">
        <Label htmlFor={`${id}-secret`}>{keyOnly ? `New ${noun.toLowerCase()}` : noun}</Label>
        <Input
          id={`${id}-secret`}
          data-slot="secret"
          type="password"
          value={secret}
          autoComplete="new-password"
          autoCapitalize="none"
          spellCheck={false}
          aria-describedby={`${id}-secret-help`}
          onChange={(e) => setSecret(e.target.value)}
        />
        <p id={`${id}-secret-help`} className="text-xs text-muted-foreground">
          Kept sealed on the server and never shown again. The agent never sees it.
        </p>
      </div>

      {problem ? (
        <InlineStatus tone="error" role="alert">
          {problem}
        </InlineStatus>
      ) : null}
      {why ? <TrustedNote>{why}</TrustedNote> : null}

      <Button type="submit" className="self-start" disabled={pending || !complete || why !== null}>
        {pending ? "Saving…" : keyOnly ? "Save the key" : "Save"}
      </Button>
    </form>
  );
}
