"use client";

import { BanIcon, CircleAlertIcon, CircleCheckIcon } from "lucide-react";
import { useState } from "react";
import { InlineStatus } from "@/components/inline-status";
import { Button } from "@/components/ui/button";
import type { ApiUserToolAuthorization, ApiUserToolServer } from "@/lib/api/types";
import type { UserToolServersView } from "../../hooks/use-user-tool-servers";
import {
  buildChange,
  hostOf,
  type Kind,
  kindLabel,
  nextAction,
  type ServerFields,
  standing,
  takesSecret,
} from "../../lib/user-servers";
import { ServerIcon } from "../server-icon";
import { ConnectDialog } from "./connect-dialog";
import { DeleteServerDialog } from "./delete-server-dialog";
import { type FormOutcome, ServerForm } from "./server-form";

type Props = {
  server: ApiUserToolServer;
  kinds: readonly Kind[];
  /** Whether `tools.user.add` is theirs: without it the only thing left to do is delete. */
  canChange: boolean;
  actions: Pick<UserToolServersView, "update" | "remove" | "authorize" | "reload">;
};

const TONE = {
  ok: { icon: CircleCheckIcon, className: "text-success" },
  action: { icon: CircleAlertIcon, className: "text-warning" },
  blocked: { icon: BanIcon, className: "text-muted-foreground" },
} as const;

/**
 * One of the person's own servers: its name, where it is (the host, never the path, which can hold a secret), the kind
 * of authentication and its **status in words** (an icon and a word, never a colour alone), then what the person can do
 * about it: Connect or give a key when it needs one, Edit, Replace key, Delete. Every change carries the `revision` the
 * row was read at; a stale one is the server's 409 and is told in words, with the list read again.
 */
export function ServerRow({ server, kinds, canChange, actions }: Props) {
  // The form is opened on the row as it is then, and keeps it: a re-read of the list that changes the row under an open form
  // must not give the form's old values the new revision, so a save diffs against, and sends the revision of, what was opened
  const [panel, setPanel] = useState<{ mode: "edit" | "key"; base: ApiUserToolServer } | null>(
    null,
  );
  const [deleting, setDeleting] = useState(false);
  const [flow, setFlow] = useState<ApiUserToolAuthorization | null>(null);
  // the revision the row had when the person went to sign in: while it is the same and the server is not ready, they are still at it
  const [waiting, setWaiting] = useState<string | null>(null);
  const [connecting, setConnecting] = useState(false);
  const [failure, setFailure] = useState<string | null>(null);
  const status = standing(server);
  const tone = TONE[status.tone];
  const Tone = tone.icon;
  const action = nextAction(server);
  const oauth = server.auth.kind === "oauth2";
  const keyed = takesSecret(server.auth.kind);

  async function connect() {
    setFailure(null);
    setConnecting(true);
    const result = await actions.authorize(server.id);
    setConnecting(false);
    if (result.ok) setFlow(result.value);
    else setFailure(result.failure.message);
  }

  async function save(fields: ServerFields): Promise<FormOutcome> {
    if (!panel) return { ok: true };
    const body = buildChange(panel.base, fields);
    if (body === null) {
      setPanel(null);
      return { ok: true };
    }
    const result = await actions.update(server.id, body);
    if (result.ok) {
      setPanel(null);
      setFailure(null);
      return { ok: true };
    }
    return {
      ok: false,
      message: result.failure.message,
      ...(result.failure.field ? { field: result.failure.field } : {}),
    };
  }

  return (
    <li
      data-slot="server-row"
      data-server={server.id}
      data-status={server.status}
      className="flex flex-col gap-3 rounded-xl border bg-card px-4 py-4 text-card-foreground"
    >
      <div className="flex min-w-0 items-start gap-3">
        <ServerIcon server={undefined} className="mt-1 size-5" />
        <div className="flex min-w-0 flex-1 flex-col gap-1">
          <h3 className="text-base leading-6 font-medium break-words">{server.name}</h3>
          <p className="text-xs text-muted-foreground">
            {kindLabel(server.auth.kind)} · <span className="break-all">{hostOf(server.url)}</span>
          </p>
          <p data-slot="status" className={`flex items-start gap-1.5 text-sm ${tone.className}`}>
            <Tone aria-hidden="true" className="mt-0.5 size-4 shrink-0" />
            <span>
              <span className="font-medium">{status.label}.</span>{" "}
              <span className="text-muted-foreground">{status.hint}</span>
            </span>
          </p>
        </div>
      </div>
      {waiting === server.revision && server.status !== "ready" ? (
        // said so it is true whether or not a window opened (a pop-up blocker is not something a page can see)
        <div
          role="status"
          data-slot="waiting"
          className="my-1 flex flex-wrap items-baseline gap-x-3 text-sm text-muted-foreground"
        >
          <span>Waiting for you to finish signing in. This list updates when you come back.</span>
          <Button
            type="button"
            variant="link"
            size="sm"
            className="h-auto px-0 py-1"
            onClick={() => actions.reload()}
          >
            Check now
          </Button>
          <Button
            type="button"
            variant="link"
            size="sm"
            className="h-auto px-0 py-1"
            disabled={connecting}
            onClick={connect}
          >
            Open the sign-in page again
          </Button>
        </div>
      ) : null}
      {failure ? (
        <InlineStatus tone="error" role="alert">
          {failure}
        </InlineStatus>
      ) : null}
      {panel ? (
        <div className="rounded-lg bg-muted/50 p-3">
          <ServerForm
            mode={panel.mode}
            kinds={kinds}
            server={panel.base}
            onSubmit={save}
            onCancel={() => setPanel(null)}
          />
        </div>
      ) : (
        <div className="flex flex-wrap gap-2">
          {canChange && action === "connect" ? (
            <Button
              type="button"
              size="sm"
              disabled={connecting}
              aria-label={`Connect ${server.name}`}
              onClick={connect}
            >
              {connecting ? "Connecting…" : "Connect"}
            </Button>
          ) : null}
          {canChange && action === "key" ? (
            <Button
              type="button"
              size="sm"
              aria-label={`Add a key to ${server.name}`}
              onClick={() => setPanel({ mode: "key", base: server })}
            >
              Add key
            </Button>
          ) : null}
          {canChange && oauth && action !== "connect" ? (
            <Button
              type="button"
              size="sm"
              variant="outline"
              disabled={connecting}
              aria-label={`Connect ${server.name} again`}
              onClick={connect}
            >
              Connect again
            </Button>
          ) : null}
          {canChange && keyed && action !== "key" ? (
            <Button
              type="button"
              size="sm"
              variant="outline"
              aria-label={`Replace the key of ${server.name}`}
              onClick={() => setPanel({ mode: "key", base: server })}
            >
              Replace key
            </Button>
          ) : null}
          {canChange ? (
            <Button
              type="button"
              size="sm"
              variant="outline"
              aria-label={`Edit ${server.name}`}
              onClick={() => setPanel({ mode: "edit", base: server })}
            >
              Edit
            </Button>
          ) : null}
          <Button
            type="button"
            size="sm"
            variant="ghost"
            aria-label={`Delete ${server.name}`}
            onClick={() => setDeleting(true)}
          >
            Delete
          </Button>
        </div>
      )}
      {flow ? (
        <ConnectDialog
          server={server}
          flow={flow}
          onClose={() => setFlow(null)}
          onOpened={() => {
            setFlow(null);
            setWaiting(server.revision);
          }}
        />
      ) : null}
      {deleting ? (
        <DeleteServerDialog
          server={server}
          onDelete={actions.remove}
          onClose={() => setDeleting(false)}
        />
      ) : null}
    </li>
  );
}
