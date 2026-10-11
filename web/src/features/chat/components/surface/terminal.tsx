"use client";

import { CheckCircle2Icon, XCircleIcon } from "lucide-react";
import { type ReactNode, useId, useState } from "react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { exitStatus, foldOutput, visibleControls } from "@/features/chat/lib/a2ui/code";
import { plainName } from "@/features/chat/lib/files";

/*
 * The `Terminal` component of the UI catalog (docs/api/ui-catalog-v1.md, version 6): a command as a shell
 * prompt, its output and its exit code. The exit code is said in words ("Exit 2 · failed"), with an icon, never
 * by colour alone. An output of more than `FOLD_LINES` lines folds behind a button that says how many lines
 * there are and holds aria-expanded; the folded part is one click away and nothing runs. All text is the agent's.
 */

type Rec = Record<string, unknown>;
const str = (v: unknown): string | undefined => (typeof v === "string" ? v : undefined);
const num = (v: unknown): number | undefined => (typeof v === "number" ? v : undefined);

function ExitBadge({ code }: { code: number | undefined }) {
  const status = exitStatus(code);
  if (status.kind === "unknown") return null;
  const ok = status.kind === "ok";
  const Icon = ok ? CheckCircle2Icon : XCircleIcon;
  return (
    <Badge
      variant={ok ? "secondary" : "destructive"}
      data-slot="terminal-exit"
      data-status={status.kind}
      className="h-auto gap-1 font-mono whitespace-nowrap"
    >
      <Icon aria-hidden="true" className="size-3" />
      {`Exit ${status.code} · ${ok ? "passed" : "failed"}`}
    </Badge>
  );
}

/** The Terminal of a surface; the props are those of the lowered component (`prepare.ts`). */
export function TerminalView(props: Rec): ReactNode {
  // raw text: a command may span lines (a heredoc), and React draws it as text
  const command = visibleControls(str(props.command) ?? "");
  const rawOutput = str(props.output);
  const output = rawOutput === undefined ? undefined : visibleControls(rawOutput);
  const title = plainName(str(props.title) ?? "", 120);
  const exitCode = num(props.exitCode);
  const folded = foldOutput(output ?? "");
  const [open, setOpen] = useState(false);
  const outputId = useId();
  const shown = folded.folded && !open ? folded.shown : (output ?? "");
  return (
    <figure
      data-slot="terminal"
      className="m-0 flex min-w-0 flex-col overflow-hidden rounded-xl border bg-muted/40"
    >
      <figcaption className="flex min-w-0 flex-wrap items-center gap-x-3 gap-y-1 border-b px-3 py-2 text-xs text-muted-foreground">
        {title ? (
          <span className="text-sm font-medium text-foreground [overflow-wrap:anywhere]">
            {title}
          </span>
        ) : null}
        <span className="ml-auto">
          <ExitBadge code={exitCode} />
        </span>
      </figcaption>
      <div className="flex min-w-0 items-start gap-2 px-3 pt-2.5 font-mono text-xs leading-relaxed">
        <span aria-hidden="true" className="select-none text-muted-foreground">
          $
        </span>
        <span className="sr-only">Command: </span>
        <code
          data-slot="terminal-command"
          className="min-w-0 font-medium whitespace-pre-wrap [overflow-wrap:anywhere]"
        >
          {command}
        </code>
      </div>
      {output !== undefined && output !== "" ? (
        // biome-ignore lint/a11y/useSemanticElements: a scrollable box, not a section of the page
        <pre
          id={outputId}
          // biome-ignore lint/a11y/noNoninteractiveTabindex: a scrollable region must be reachable by keyboard
          tabIndex={0}
          role="region"
          aria-label="Output"
          data-slot="terminal-output"
          className="m-0 max-h-96 overflow-auto px-3 pt-2 pb-3 font-mono text-xs leading-relaxed whitespace-pre-wrap [overflow-wrap:anywhere]"
        >
          {shown}
        </pre>
      ) : null}
      {folded.folded ? (
        <div className="px-3 pb-2.5">
          <Button
            type="button"
            variant="ghost"
            size="sm"
            className="h-7 px-2 text-xs"
            aria-expanded={open}
            aria-controls={outputId}
            onClick={() => setOpen((o) => !o)}
          >
            {open ? "Show less" : `Show all ${folded.lines} lines`}
          </Button>
        </div>
      ) : null}
    </figure>
  );
}
