"use client";

import {
  ArrowUpRightIcon,
  CircleCheckIcon,
  CircleXIcon,
  DownloadIcon,
  FileTextIcon,
  GitBranchIcon,
  GitPullRequestIcon,
  LinkIcon,
  type LucideIcon,
} from "lucide-react";
import { useState } from "react";
import { Dialog, DialogContent, DialogDescription, DialogTitle } from "@/components/ui/dialog";
import { useObjectUrl } from "@/features/chat/hooks/use-object-url";
import { fetchFileBlob, mustFetch, saveBlob } from "@/features/chat/lib/file-access";
import { useBrowserAuth } from "@/lib/auth/use-browser-auth";
import { cn } from "@/lib/utils";
import { KIND_LABEL, type Source, type SourceGroup, type SourceKind } from "../lib/sources";
import { EmptyPanel } from "./empty-panel";

const LINK_CLASS =
  "inline cursor-pointer text-start text-sm leading-5 font-medium text-foreground [overflow-wrap:anywhere] underline decoration-muted-foreground/50 underline-offset-2 hover:decoration-foreground";

/**
 * A kept file where a link cannot carry the session's token (ADR 0054, decision 9): the file is
 * fetched, and "open" shows an image inside the app (a dialog with an `<img src=blob:>`) and saves
 * anything else. A `blob:` URL is never navigated to: it has the page's origin and none of the
 * server's headers.
 */
function OpenFile({ source }: { source: Source }) {
  const [open, setOpen] = useState(false);
  const image = source.preview === "image";
  const object = useObjectUrl(source.href ?? "", open && image);
  return (
    <>
      <button
        type="button"
        data-slot="source-open"
        className={LINK_CLASS}
        onClick={() => {
          if (image) setOpen(true);
          else if (source.downloadHref) {
            fetchFileBlob(source.downloadHref).then(
              (blob) => saveBlob(blob, source.title),
              () => {},
            );
          }
        }}
      >
        {source.title}
        <ArrowUpRightIcon aria-hidden="true" className="ms-0.5 inline size-3.5 align-text-top" />
        <span className="sr-only">{image ? " (opens here)" : " (downloads)"}</span>
      </button>
      {image ? (
        <Dialog open={open} onOpenChange={setOpen}>
          <DialogContent className="w-[min(60rem,calc(100vw-2rem))]">
            <DialogTitle className="pe-8 text-sm font-medium [overflow-wrap:anywhere]">
              {source.title}
            </DialogTitle>
            <DialogDescription className="sr-only">An image the agents shared.</DialogDescription>
            {object.state === "ready" ? (
              // biome-ignore lint/performance/noImgElement: an object URL of a file the API served; next/image would proxy it
              <img
                data-slot="source-lightbox-image"
                src={object.url}
                alt={source.title}
                className="max-h-[70dvh] max-w-full justify-self-center rounded-lg border bg-muted/30 object-contain"
              />
            ) : (
              <p className="text-xs text-muted-foreground" role="status">
                {object.state === "error" ? "The image could not be shown." : "Loading the image…"}
              </p>
            )}
          </DialogContent>
        </Dialog>
      ) : null}
    </>
  );
}

const ICON: Record<SourceKind, LucideIcon> = {
  pull_request: GitPullRequestIcon,
  branch: GitBranchIcon,
  ci: CircleCheckIcon,
  file: FileTextIcon,
  link: LinkIcon,
};

/** How many "Turn n" buttons an item shows; a source cited more often keeps the first ones. */
const TURNS_SHOWN = 3;

/** The words of a source: its title as a link (when it has a place to go) and what it is. */
function SourceRow({
  source,
  onShowTurn,
}: {
  source: Source;
  onShowTurn: (turnId: string) => void;
}) {
  const Icon = source.kind === "ci" && source.passed === false ? CircleXIcon : ICON[source.kind];
  const kept = source.kind === "file" && source.downloadHref !== undefined && source.href;
  const cfg = useBrowserAuth();
  const fetched = !!kept && mustFetch(cfg, source.href as string);
  const shownTurns = source.turns.slice(0, TURNS_SHOWN);
  const more = source.turns.length - shownTurns.length;
  return (
    <li
      data-slot="source"
      data-kind={source.kind}
      className="flex items-start gap-3 rounded-xl px-2 py-2.5 hover:bg-muted/60"
    >
      <span
        aria-hidden="true"
        className={cn(
          "mt-0.5 flex size-8 shrink-0 items-center justify-center rounded-full bg-muted text-muted-foreground",
          source.kind === "ci" && (source.passed ? "text-success" : "text-destructive"),
        )}
      >
        <Icon className="size-4" />
      </span>
      <div className="min-w-0 flex-1">
        {fetched ? (
          <OpenFile source={source} />
        ) : source.href ? (
          <a
            href={source.href}
            target="_blank"
            rel="noopener noreferrer"
            className="inline text-sm leading-5 font-medium text-foreground [overflow-wrap:anywhere] underline decoration-muted-foreground/50 underline-offset-2 hover:decoration-foreground"
          >
            {source.title}
            <ArrowUpRightIcon
              aria-hidden="true"
              className="ms-0.5 inline size-3.5 align-text-top"
            />
            <span className="sr-only"> (opens in a new tab)</span>
          </a>
        ) : (
          <p className="text-sm leading-5 font-medium [overflow-wrap:anywhere]">{source.title}</p>
        )}
        <p className="mt-0.5 truncate text-xs text-muted-foreground">
          {KIND_LABEL[source.kind]}
          {source.detail ? ` · ${source.detail}` : ""}
        </p>
      </div>
      <div className="flex shrink-0 flex-wrap items-center justify-end gap-1">
        {fetched && source.downloadHref ? (
          <button
            type="button"
            data-slot="source-download"
            aria-label={`Download ${source.title}`}
            onClick={() => {
              fetchFileBlob(source.downloadHref as string).then(
                (blob) => saveBlob(blob, source.title),
                () => {},
              );
            }}
            className="inline-flex size-7 cursor-pointer items-center justify-center rounded-full border text-muted-foreground transition-colors hover:bg-background hover:text-foreground"
          >
            <DownloadIcon aria-hidden="true" className="size-3.5" />
          </button>
        ) : source.downloadHref ? (
          <a
            data-slot="source-download"
            href={source.downloadHref}
            download
            aria-label={`Download ${source.title}`}
            className="inline-flex size-7 items-center justify-center rounded-full border text-muted-foreground transition-colors hover:bg-background hover:text-foreground"
          >
            <DownloadIcon aria-hidden="true" className="size-3.5" />
          </a>
        ) : null}
        {shownTurns.map((turn) => (
          <button
            key={turn.id}
            type="button"
            aria-label={`Show turn ${turn.number} in the conversation`}
            onClick={() => onShowTurn(turn.id)}
            className="h-7 min-w-12 cursor-pointer rounded-full border px-2.5 text-xs text-muted-foreground transition-colors hover:bg-background hover:text-foreground"
          >
            Turn {turn.number}
          </button>
        ))}
        {more > 0 ? (
          <span className="inline-flex h-7 items-center text-xs text-muted-foreground">
            +{more}
          </span>
        ) : null}
      </div>
    </li>
  );
}

/** The turns the sources are read from, when the thread was opened at its end and older turns are not loaded (ADR 0059). */
export type SourcesWindow = {
  /** The agent turns held. */
  turns: number;
  state: "idle" | "loading" | "waiting" | "error";
  error: string | null;
  /** Loads every older turn; the sources of all of them are then listed. */
  onLoadAll: () => void;
};

/** Says that the sources are those of the last turns, and offers the rest: never loaded unasked, a long thread is many pages. */
function WindowNote({ window }: { window: SourcesWindow }) {
  const { turns, state, error, onLoadAll } = window;
  return (
    <div
      data-slot="sources-window"
      data-state={state}
      className="flex flex-col items-start gap-1.5 rounded-xl border border-dashed px-3 py-2.5 text-xs text-muted-foreground"
    >
      <p>
        {turns > 0
          ? `Sources from the last ${turns} ${turns === 1 ? "turn" : "turns"}. Earlier ones are not loaded.`
          : "Earlier turns are not loaded."}
      </p>
      {state === "loading" ? (
        <p role="status">Loading earlier turns…</p>
      ) : state === "waiting" ? (
        <p role="status">Earlier turns will load when the agent is done.</p>
      ) : (
        <>
          {state === "error" ? (
            <p role="alert">Could not load earlier turns{error ? `: ${error}` : ""}.</p>
          ) : null}
          <button
            type="button"
            data-slot="sources-load-all"
            onClick={onLoadAll}
            className="h-7 cursor-pointer rounded-full border px-3 text-xs text-foreground transition-colors hover:bg-muted"
          >
            {state === "error" ? "Try again" : "Load earlier turns"}
          </button>
        </>
      )}
    </div>
  );
}

/**
 * The Sources tab, from the groups of `collectSources`: pull requests and branches, checks, files,
 * links, each item once with the turns that cited it. Props only, so it is tested without a runtime.
 */
export function SourcesView({
  groups,
  onShowTurn,
  window,
}: {
  groups: readonly SourceGroup[];
  onShowTurn: (turnId: string) => void;
  /** Present while older turns are not loaded. */
  window?: SourcesWindow;
}) {
  if (groups.length === 0) {
    return (
      <>
        <EmptyPanel title="Nothing shared yet">
          Links, files and pull requests the agents share show up here.
        </EmptyPanel>
        {window ? (
          <div className="px-3 pb-3">
            <WindowNote window={window} />
          </div>
        ) : null}
      </>
    );
  }
  return (
    <div className="flex flex-col gap-5 p-3">
      {window ? <WindowNote window={window} /> : null}
      {groups.map((group) => (
        <section key={group.id} aria-labelledby={`sources-${group.id}`}>
          <h3
            id={`sources-${group.id}`}
            className="px-2 pb-1 text-xs font-medium text-muted-foreground"
          >
            {group.label}
          </h3>
          <ul className="flex flex-col">
            {group.items.map((source) => (
              <SourceRow key={source.key} source={source} onShowTurn={onShowTurn} />
            ))}
          </ul>
        </section>
      ))}
    </div>
  );
}
