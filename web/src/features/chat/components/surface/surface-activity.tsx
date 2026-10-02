"use client";

import { useAuiState } from "@assistant-ui/react";
import { CircleAlertIcon, RefreshCwIcon } from "lucide-react";
import { type ReactNode, useMemo } from "react";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { prepareSurface } from "@/features/chat/lib/a2ui/prepare";
import { latestSurface } from "@/features/chat/lib/a2ui/surfaces";
import { parseSurface, type SurfaceContent } from "@/features/chat/lib/agui/vymalo";
import { useThreadFilesList } from "../../hooks/use-thread-files";
import { ActorLabel } from "../actor-label";
import { useThreadView } from "../thread-view";
import { SurfaceView } from "./surface-view";
import { ThreadFilesContext } from "./thread-files";

/** How much of the raw operations a refusal shows: they are the agent's, and can be large. */
const RAW_SHOWN = 4000;

const rawOperations = (operations: unknown): string => {
  let json: string;
  try {
    json = JSON.stringify(operations, null, 2) ?? "";
  } catch {
    return "(not JSON)";
  }
  return json.length > RAW_SHOWN ? `${json.slice(0, RAW_SHOWN)}\n…` : json;
};

/** The whole surface is refused: what rule it broke, and the operations, as text. */
export function SurfaceRefused({
  rule,
  reason,
  operations,
  actor,
}: {
  rule: string;
  reason: string;
  operations: unknown;
  actor: SurfaceContent["actor"];
}) {
  return (
    <Alert variant="destructive" role="none" className="max-w-xl" data-rule={rule}>
      <CircleAlertIcon aria-hidden="true" />
      <AlertTitle className="[overflow-wrap:anywhere]">
        <strong>Interface not shown:</strong> {reason}
      </AlertTitle>
      <AlertDescription className="flex flex-col gap-2">
        <span className="flex flex-wrap items-baseline justify-between gap-x-3">
          <span>The agent sent an interface this app does not draw, so none of it is.</span>
          <ActorLabel actor={actor} />
        </span>
        <details>
          <summary className="cursor-pointer text-sm underline underline-offset-2">
            Raw operations
          </summary>
          <pre
            // biome-ignore lint/a11y/noNoninteractiveTabindex: a scrollable region must be reachable by keyboard
            tabIndex={0}
            className="mt-2 max-h-64 overflow-auto rounded-md bg-muted p-2 text-[0.8125rem] whitespace-pre-wrap text-foreground [overflow-wrap:anywhere]"
          >
            {rawOperations(operations)}
          </pre>
        </details>
      </AlertDescription>
    </Alert>
  );
}

/** A name from an agent, as one short line of text. */
// biome-ignore lint/suspicious/noControlCharactersInRegex: stripping them is the point
const CONTROL = /[\u0000-\u001f\u007f-\u009f]/g;
const shown = (name: string) => name.replace(CONTROL, "?").slice(0, 40);

/**
 * A surface of this app's catalog that names a component of a newer version of it (ADR 0023, D3):
 * the thread was opened in a newer version of the app, and this one cannot draw what the agent
 * sent. Said out loud, never half drawn (ADR 0013 rule 4); a reload picks up a newer build.
 */
export function SurfaceNewer({
  component,
  actor,
  onReload = () => window.location.reload(),
}: {
  component: string;
  actor: SurfaceContent["actor"];
  onReload?: () => void;
}) {
  return (
    <Alert
      role="group"
      data-slot="surface-newer"
      className="max-w-xl"
      aria-label="Interface needs a newer version of the app"
    >
      <RefreshCwIcon aria-hidden="true" />
      <AlertTitle>This part of the answer needs a newer version of the app.</AlertTitle>
      <AlertDescription className="flex flex-col gap-2">
        <span className="[overflow-wrap:anywhere]">
          The agent used “{shown(component)}”, which this version does not have.
        </span>
        <span className="flex flex-wrap items-center justify-between gap-x-3 gap-y-2">
          <Button type="button" size="sm" variant="outline" onClick={onReload}>
            Reload
          </Button>
          <ActorLabel actor={actor} />
        </span>
      </AlertDescription>
    </Alert>
  );
}

/**
 * An A2UI surface (ADR 0013): the operations pass `prepareSurface`, and only a surface that
 * passes is drawn, whole. It is labelled by the orchestrator's `vymalo.actor` and by nothing the
 * agent wrote (a theme's name or icon is never drawn). Only the newest copy of a surface is live.
 */
export function SurfaceActivity({ data }: { data: unknown }) {
  const content = parseSurface(data);
  const operations = content?.a2ui_operations;
  // the newest catalog the thread has seen: a component this build lacks may be one of a newer one
  const { catalogVersion } = useThreadView();
  // the files the thread holds: an Image may name one of them and nothing else (ADR 0032)
  const files = useThreadFilesList();
  const options = useMemo(
    () => ({ threadVersion: catalogVersion, files }),
    [catalogVersion, files],
  );
  const prepared = useMemo(() => prepareSurface(operations, options), [operations, options]);
  const key = content?.surface ?? "";
  const messageId = useAuiState((s) => s.message.id);
  const latest = useAuiState((s) => latestSurface(s.thread.messages, key)?.data);
  const latestId = useAuiState((s) => latestSurface(s.thread.messages, key)?.messageId);
  const newer = useMemo(() => {
    const ops = parseSurface(latest)?.a2ui_operations;
    return latest === undefined ? undefined : prepareSurface(ops, options);
  }, [latest, options]);
  if (!content) return null;

  const live = latestId === undefined || latestId === messageId;
  const label = `Interface from ${content.actor?.name ?? "the agent"}`;
  let body: ReactNode;
  if (!live) {
    // updated (or deleted) in a later run: the newer copy is where the surface is now
    body =
      newer?.kind === "deleted" ? null : (
        <p className="text-xs text-muted-foreground">This interface was updated further down.</p>
      );
  } else if (prepared.kind === "deleted") {
    body = null;
  } else if (prepared.kind === "refused") {
    return (
      <SurfaceRefused
        rule={prepared.rule}
        reason={prepared.reason}
        operations={operations}
        actor={content.actor}
      />
    );
  } else if (prepared.kind === "newer") {
    return <SurfaceNewer component={prepared.component} actor={content.actor} />;
  } else if (prepared.kind === "pending") {
    body = <p className="text-xs text-muted-foreground">The interface is not complete yet.</p>;
  } else {
    body = (
      <ThreadFilesContext.Provider value={files}>
        <SurfaceView
          prepared={prepared}
          live={live}
          fallback={
            <SurfaceRefused
              rule="render"
              reason="the interface failed to draw"
              operations={operations}
              actor={content.actor}
            />
          }
        />
      </ThreadFilesContext.Provider>
    );
  }
  if (body === null) return null;
  return (
    <section
      aria-label={label}
      data-slot="a2ui-surface"
      className="flex w-full max-w-xl min-w-0 flex-col gap-1 rounded-lg border bg-card p-3 text-card-foreground"
    >
      {body}
      <ActorLabel actor={content.actor} />
    </section>
  );
}
