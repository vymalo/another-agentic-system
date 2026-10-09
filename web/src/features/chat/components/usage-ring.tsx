"use client";

import { useMemo, useState } from "react";
import { Hint } from "@/components/hint";
import { Button } from "@/components/ui/button";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { useAgentNames } from "@/features/agents/components/agent-names-context";
import {
  type FillLevel,
  hasUsage,
  type ModelCounts,
  summarize,
  type ThreadUsage,
  type UsageGroup,
} from "@/features/chat/lib/usage";

const number = new Intl.NumberFormat("en-US");
const n = (v: number) => number.format(v);
const percent = (ratio: number) => `${Math.round(ratio * 100)} %`;

const ARC: Record<FillLevel, string> = {
  normal: "text-muted-foreground",
  warn: "text-warning",
  danger: "text-destructive",
};

const KIND: Record<UsageGroup["kind"], string> = {
  agent: "Agent",
  subagent: "Sub-agent",
  ask: "Asked agent",
};

const R = 8;
const C = 2 * Math.PI * R;

/**
 * The thread's token usage (ADR 0056), beside the send button: a ring that fills with how full the
 * context of the agent's latest call is (neutral below 80 %, amber from 80 %, red from 95 %; no fill
 * when the call said no window), and a popover with the thread's totals per model and the calls of
 * the agent, of each sub-agent and of each asked agent apart. Nothing when the thread has no usage.
 */
export function UsageRing({ usage }: { usage: ThreadUsage }) {
  const names = useAgentNames();
  const s = useMemo(() => summarize(usage), [usage]);
  const [open, setOpen] = useState(false);
  if (!hasUsage(usage)) return null;
  const { latest, fill } = s;
  const nameOf = (g: UsageGroup) =>
    g.kind === "subagent" ? g.name : (names.get(g.name) ?? g.name);
  const context =
    latest && fill && latest.contextWindow
      ? `context ${percent(fill.ratio)} full, ${n(latest.counts.inputTokens)} of ${n(latest.contextWindow)} tokens`
      : latest
        ? `last call ${n(latest.counts.inputTokens)} input tokens, no context window known`
        : "no call of the agent yet";
  return (
    <Popover onOpenChange={setOpen}>
      <Hint label="Token usage" side="top" suppressed={open}>
        <PopoverTrigger asChild>
          <Button
            type="button"
            variant="ghost"
            aria-label={`Token usage: ${context}`}
            data-level={fill?.level ?? "none"}
            className="size-9 shrink-0 rounded-full p-0 text-muted-foreground"
          >
            <svg viewBox="0 0 20 20" className="size-5 -rotate-90" aria-hidden="true">
              <circle
                cx="10"
                cy="10"
                r={R}
                fill="none"
                strokeWidth="2.5"
                className="stroke-border"
              />
              {fill ? (
                <circle
                  data-slot="usage-fill"
                  cx="10"
                  cy="10"
                  r={R}
                  fill="none"
                  strokeWidth="2.5"
                  strokeLinecap="round"
                  stroke="currentColor"
                  strokeDasharray={`${Math.max(fill.ratio * C, 0.5)} ${C}`}
                  className={ARC[fill.level]}
                />
              ) : null}
            </svg>
          </Button>
        </PopoverTrigger>
      </Hint>
      <PopoverContent
        side="top"
        align="end"
        aria-label="Token usage"
        className="w-80 max-w-[calc(100vw-2rem)] text-sm"
      >
        <p className="font-medium">Token usage</p>
        <p className="mt-0.5 text-xs text-muted-foreground">
          {latest && fill && latest.contextWindow
            ? `The agent's last call filled ${percent(fill.ratio)} of the context: ${n(latest.counts.inputTokens)} of ${n(latest.contextWindow)} tokens (${latest.model}).`
            : latest
              ? `The agent's last call read ${n(latest.counts.inputTokens)} tokens (${latest.model}); its context window is not known.`
              : "The agent has made no call yet."}
        </p>
        <UsageTable title="This thread, by model" rows={s.models.map(modelRow)} />
        <UsageTable
          title="By who spent it"
          rows={s.groups.map((g) => ({
            key: `${g.kind}:${g.name}`,
            label: nameOf(g),
            detail: `${KIND[g.kind]} · ${g.calls} ${g.calls === 1 ? "call" : "calls"}`,
            counts: g.counts,
          }))}
        />
      </PopoverContent>
    </Popover>
  );
}

type Row = { key: string; label: string; detail?: string; counts: ModelCounts["counts"] };

const modelRow = (m: ModelCounts): Row => ({
  key: `${m.provider ?? ""}/${m.model}`,
  label: m.model,
  ...(m.provider ? { detail: m.provider } : {}),
  counts: m.counts,
});

function UsageTable({ title, rows }: { title: string; rows: Row[] }) {
  if (rows.length === 0) return null;
  return (
    <table className="mt-3 w-full table-fixed text-xs">
      <caption className="mb-1 text-start text-xs font-medium text-muted-foreground">
        {title}
      </caption>
      <thead className="sr-only">
        <tr>
          <th scope="col">Name</th>
          <th scope="col">Input</th>
          <th scope="col">Output</th>
        </tr>
      </thead>
      <tbody>
        {rows.map((r) => (
          <tr key={r.key} className="border-t border-border/60 align-top">
            <th scope="row" className="w-[44%] py-1 pe-2 text-start font-normal">
              <span className="block truncate font-medium">{r.label}</span>
              {r.detail ? <span className="block text-muted-foreground">{r.detail}</span> : null}
            </th>
            <td className="py-1 pe-2 tabular-nums">
              <span className="block">{n(r.counts.inputTokens)} in</span>
              {r.counts.cachedInputTokens !== undefined ? (
                <span className="block text-muted-foreground">
                  {n(r.counts.cachedInputTokens)} cached
                </span>
              ) : null}
            </td>
            <td className="py-1 tabular-nums">
              <span className="block">{n(r.counts.outputTokens)} out</span>
              {r.counts.reasoningTokens !== undefined ? (
                <span className="block text-muted-foreground">
                  {n(r.counts.reasoningTokens)} reasoning
                </span>
              ) : null}
            </td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}
