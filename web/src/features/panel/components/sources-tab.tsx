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
import { cn } from "@/lib/utils";
import { KIND_LABEL, type Source, type SourceGroup, type SourceKind } from "../lib/sources";
import { EmptyPanel } from "./empty-panel";

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
        {source.href ? (
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
        {source.downloadHref ? (
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

/**
 * The Sources tab, from the groups of `collectSources`: pull requests and branches, checks, files,
 * links, each item once with the turns that cited it. Props only, so it is tested without a runtime.
 */
export function SourcesView({
  groups,
  onShowTurn,
}: {
  groups: readonly SourceGroup[];
  onShowTurn: (turnId: string) => void;
}) {
  if (groups.length === 0) {
    return (
      <EmptyPanel title="Nothing shared yet">
        Links, files and pull requests the agents share show up here.
      </EmptyPanel>
    );
  }
  return (
    <div className="flex flex-col gap-5 p-3">
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
