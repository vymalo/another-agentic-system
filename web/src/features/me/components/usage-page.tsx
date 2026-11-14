"use client";

import { ArrowLeftIcon, ChevronLeftIcon, ChevronRightIcon } from "lucide-react";
import Link from "next/link";
import { useState } from "react";
import { InlineStatus, LoadingStatus } from "@/components/inline-status";
import { Button } from "@/components/ui/button";
import { useMyUsage } from "@/features/me/hooks/use-my-usage";
import { currentMonth, monthLabel, nextMonth, previousMonth } from "@/features/me/lib/month";
import { readAnswer } from "@/features/me/lib/usage-answer";
import { openSignIn } from "@/lib/api/session";
import type { ApiMyUsage, ApiUsageModelLine, ApiUsageSandboxLine } from "@/lib/api/types";

/*
 * ADR 0065: what the person used in a UTC month, as `GET /api/me/usage` priced it. Tokens and sandbox time are always
 * shown; money only when the answer carries a currency. An amount is a string the API wrote and is drawn as it is: it
 * is never parsed into a number, so nothing is rounded or lost. A total that is not complete is "at least", and the page
 * names what it leaves out.
 */

const number = new Intl.NumberFormat("en-US");
const n = (v: number) => number.format(v);
const plural = (count: number, one: string, many = `${one}s`) =>
  `${n(count)} ${count === 1 ? one : many}`;

/** Milliseconds as the two largest units that matter: `1 h 12 min`, `3 min 20 s`, `45 s`, `850 ms`. */
function duration(ms: number): string {
  if (ms < 1000) return `${n(ms)} ms`;
  const s = Math.round(ms / 1000);
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  if (h > 0) return `${n(h)} h ${m} min`;
  if (m > 0) return `${m} min ${s % 60} s`;
  return `${s} s`;
}

const trimmed = (v: number) => String(Number(v.toFixed(2)));

/** The size a sandbox reserved: the CPU and memory its pod asked for, which no request leaves unknown. */
function shape(line: ApiUsageSandboxLine): string {
  const parts: string[] = [];
  if (line.cpuMillis !== undefined) {
    const cores = trimmed(line.cpuMillis / 1000);
    parts.push(`${cores} ${cores === "1" ? "core" : "cores"}`);
  }
  if (line.memoryBytes !== undefined) parts.push(`${trimmed(line.memoryBytes / 2 ** 30)} GiB`);
  return parts.length > 0 ? parts.join(", ") : "size not requested";
}

const modelName = (m: { provider?: string; model: string }) =>
  m.provider ? `${m.provider} / ${m.model}` : m.model;

/** The tasks whose sandbox time was not measured: what the lines and the total each say, the larger of the two. */
const unmeasuredTasks = (usage: ApiMyUsage) =>
  Math.max(
    usage.unmeasured.reduce((sum, u) => sum + u.tasks, 0),
    usage.total.unpriced.unmeasured,
  );

/** What the total leaves out, in words. */
function leftOut(usage: ApiMyUsage): string[] {
  const { unpriced } = usage.total;
  const out: string[] = [];
  if (unpriced.models.length > 0) {
    out.push(`no price for ${unpriced.models.map(modelName).join(", ")}`);
  }
  if (unpriced.sandbox > 0) out.push(`${plural(unpriced.sandbox, "sandbox shape")} with no price`);
  if (unpriced.unmeasured > 0) {
    out.push(
      `${plural(unpriced.unmeasured, "task")} ran commands where the sandbox is not measured` +
        `${usage.unmeasured.length > 0 ? ` (${usage.unmeasured.map((u) => u.environment).join(", ")})` : ""}, so their time is not counted`,
    );
  }
  return out;
}

function Cost({ price, currency }: { price: string | null; currency: string }) {
  return price === null ? (
    <span className="text-muted-foreground">Not priced</span>
  ) : (
    <>
      {price} {currency}
    </>
  );
}

function TotalCard({ usage }: { usage: ApiMyUsage }) {
  const { total } = usage;
  const out = leftOut(usage);
  return (
    <section
      aria-labelledby="usage-total"
      className="rounded-xl border bg-card px-4 py-4 text-card-foreground"
    >
      <h2 id="usage-total" className="text-sm font-medium text-muted-foreground">
        Total
      </h2>
      {usage.currency === null ? (
        <p className="mt-1 text-sm text-muted-foreground">
          No prices are configured, so no cost is shown.
        </p>
      ) : (
        <p data-slot="total" className="mt-1 text-[1.75rem] leading-tight font-medium tabular-nums">
          {total.complete ? null : <span className="text-base font-normal">At least </span>}
          <span className="break-all">
            {total.amount} {usage.currency}
          </span>
        </p>
      )}
      {usage.currency !== null && !total.complete ? (
        <div className="mt-2 text-sm text-muted-foreground">
          {out.length > 0 ? (
            <>
              <p>This is the sum of the lines that have a price, not the whole cost. Left out:</p>
              <ul className="mt-1 list-disc ps-5">
                {out.map((line) => (
                  <li key={line}>{line}</li>
                ))}
              </ul>
            </>
          ) : (
            <p>
              This is the sum of the lines that have a price, not the whole cost. The answer does
              not say which lines are left out (lines past its limit are not priced).
            </p>
          )}
        </div>
      ) : null}
      {usage.prices ? (
        <p className="mt-3 text-xs text-muted-foreground">
          Priced with today’s table <span className="font-mono">{usage.prices}</span>; a past month
          is priced the same way.
        </p>
      ) : null}
    </section>
  );
}

function Totals({ usage }: { usage: ApiMyUsage }) {
  const tokens = usage.models.reduce((sum, m) => sum + m.totalTokens, 0);
  const ms = usage.sandbox.reduce((sum, s) => sum + s.milliseconds, 0);
  const unmeasured = unmeasuredTasks(usage);
  return (
    <dl className="grid grid-cols-3 gap-3 text-sm">
      {(
        [
          ["Tasks", n(usage.tasks), null],
          ["Tokens", n(tokens), null],
          [
            "Sandbox time",
            duration(ms),
            unmeasured > 0 ? `${plural(unmeasured, "task")} not measured` : null,
          ],
        ] as const
      ).map(([label, value, note]) => (
        <div key={label} className="rounded-xl bg-muted px-3 py-2">
          <dt className="text-xs text-muted-foreground">{label}</dt>
          <dd className="mt-0.5 font-medium tabular-nums">
            {value}
            {note ? (
              <span className="block text-xs font-normal text-muted-foreground">{note}</span>
            ) : null}
          </dd>
        </div>
      ))}
    </dl>
  );
}

const th = "py-2 pe-3 text-start text-xs font-medium text-muted-foreground";
const thEnd = "py-2 text-end text-xs font-medium text-muted-foreground";
const td = "py-2 pe-3 align-top tabular-nums";

function ModelsTable({ lines, currency }: { lines: ApiUsageModelLine[]; currency: string | null }) {
  if (lines.length === 0) {
    return <p className="text-sm text-muted-foreground">No model usage this month.</p>;
  }
  return (
    <table className="w-full text-sm">
      <caption className="sr-only">Models</caption>
      <thead>
        <tr className="border-b">
          <th scope="col" className={th}>
            Model
          </th>
          <th scope="col" className={th}>
            Tokens
          </th>
          {currency !== null ? (
            <th scope="col" className={thEnd}>
              Cost
            </th>
          ) : null}
        </tr>
      </thead>
      <tbody>
        {lines.map((m, i) => (
          <tr
            // biome-ignore lint/suspicious/noArrayIndexKey: two lines may name one model; the answer's order is the identity
            key={`${m.provider ?? ""}/${m.model}/${i}`}
            className="border-b border-border/60"
          >
            <th scope="row" className="py-2 pe-3 text-start align-top font-normal">
              <span className="block font-medium break-all">{m.model}</span>
              {m.provider ? (
                <span className="block text-xs text-muted-foreground">{m.provider}</span>
              ) : null}
            </th>
            <td className={td}>
              <span className="block">{n(m.inputTokens)} in</span>
              {m.cachedInputTokens !== undefined ? (
                <span className="block text-xs text-muted-foreground">
                  {n(m.cachedInputTokens)} cached
                </span>
              ) : null}
              <span className="block">{n(m.outputTokens)} out</span>
              {m.reasoningTokens !== undefined ? (
                <span className="block text-xs text-muted-foreground">
                  {n(m.reasoningTokens)} reasoning
                </span>
              ) : null}
            </td>
            {currency !== null ? (
              <td className={`${td} pe-0 text-end break-all`}>
                <Cost price={m.price} currency={currency} />
              </td>
            ) : null}
          </tr>
        ))}
      </tbody>
    </table>
  );
}

function SandboxTable({
  lines,
  currency,
}: {
  lines: ApiUsageSandboxLine[];
  currency: string | null;
}) {
  if (lines.length === 0) {
    return <p className="text-sm text-muted-foreground">No sandbox time this month.</p>;
  }
  return (
    <table className="w-full text-sm">
      <caption className="sr-only">Sandbox</caption>
      <thead>
        <tr className="border-b">
          <th scope="col" className={th}>
            Shape
          </th>
          <th scope="col" className={th}>
            Time
          </th>
          {currency !== null ? (
            <th scope="col" className={thEnd}>
              Cost
            </th>
          ) : null}
        </tr>
      </thead>
      <tbody>
        {lines.map((s, i) => (
          <tr
            // biome-ignore lint/suspicious/noArrayIndexKey: two lines may name one shape; the answer's order is the identity
            key={`${s.environment}/${s.cpuMillis ?? ""}/${s.memoryBytes ?? ""}/${i}`}
            className="border-b border-border/60"
          >
            <th scope="row" className="py-2 pe-3 text-start align-top font-normal">
              <span className="block font-medium">{s.environment}</span>
              <span className="block text-xs text-muted-foreground">{shape(s)}</span>
            </th>
            <td className={td}>
              <span className="block">{duration(s.milliseconds)}</span>
              <span className="block text-xs text-muted-foreground">
                {plural(s.commands, "command")}
              </span>
            </td>
            {currency !== null ? (
              <td className={`${td} pe-0 text-end break-all`}>
                <Cost price={s.price} currency={currency} />
              </td>
            ) : null}
          </tr>
        ))}
      </tbody>
    </table>
  );
}

function Month({ usage, partial }: { usage: ApiMyUsage; partial: boolean }) {
  return (
    <div className="flex flex-col gap-6">
      {partial ? (
        <InlineStatus role="status">
          The answer left out part of the month, so some lines may be missing.
        </InlineStatus>
      ) : null}
      <TotalCard usage={usage} />
      <Totals usage={usage} />
      <section aria-labelledby="usage-models" className="flex flex-col gap-2">
        <h2 id="usage-models" className="text-sm font-medium">
          Models
        </h2>
        <ModelsTable lines={usage.models} currency={usage.currency} />
      </section>
      <section aria-labelledby="usage-sandbox" className="flex flex-col gap-2">
        <h2 id="usage-sandbox" className="text-sm font-medium">
          Sandbox
        </h2>
        <SandboxTable lines={usage.sandbox} currency={usage.currency} />
      </section>
    </div>
  );
}

/**
 * `/usage`: the person's own month, from the usage ledger (ADR 0065). No permission gates it: the route answers for
 * the authenticated person and for nobody else. The month buttons stay on the screen while a month loads, and
 * **Next month** stops at the current one (the future has nothing to read) by `aria-disabled`, not `disabled`, so
 * that it keeps the focus it has when it gets there. Figures are drawn only for the month the page names.
 */
export function UsagePage() {
  const now = currentMonth();
  const [month, setMonth] = useState(now);
  const read = useMyUsage(month);
  const before = previousMonth(month);
  const after = nextMonth(month);
  const noBefore = before === null;
  const noAfter = after === null || month >= now;
  const answer = read.status === "ready" ? readAnswer(read.usage) : null;
  // an answer for another month than the one on the screen is not drawn under it
  const stale = answer !== null && answer.usage.month !== month;
  return (
    <main className="mx-auto flex min-h-dvh w-full max-w-3xl flex-col gap-6 px-4 py-6 md:px-6">
      <header className="flex flex-col gap-4">
        <Button asChild variant="ghost" size="sm" className="-ms-2 self-start">
          <Link href="/">
            <ArrowLeftIcon aria-hidden="true" />
            Back to chat
          </Link>
        </Button>
        <h1 className="text-[1.75rem] leading-tight font-medium tracking-tight">Usage</h1>
        <fieldset className="flex min-w-0 items-center gap-2 border-0 p-0">
          <legend className="sr-only">Month</legend>
          <Button
            type="button"
            variant="outline"
            size="icon"
            aria-label="Previous month"
            aria-disabled={noBefore}
            className="aria-disabled:pointer-events-none aria-disabled:opacity-50"
            onClick={() => before && setMonth(before)}
          >
            <ChevronLeftIcon aria-hidden="true" />
          </Button>
          <p
            aria-live="polite"
            className="min-w-36 text-center text-sm font-medium tabular-nums"
            data-slot="month"
          >
            {monthLabel(month)}
          </p>
          <Button
            type="button"
            variant="outline"
            size="icon"
            aria-label="Next month"
            aria-disabled={noAfter}
            className="aria-disabled:pointer-events-none aria-disabled:opacity-50"
            onClick={() => !noAfter && after && setMonth(after)}
          >
            <ChevronRightIcon aria-hidden="true" />
          </Button>
        </fieldset>
        <p className="text-xs text-muted-foreground">Months are UTC calendar months.</p>
      </header>
      {read.status === "loading" || stale ? <LoadingStatus>Loading usage</LoadingStatus> : null}
      {answer !== null && !stale ? <Month usage={answer.usage} partial={answer.partial} /> : null}
      {read.status === "ready" && answer === null ? (
        <InlineStatus
          tone="error"
          role="alert"
          action={{ label: "Try again", onClick: read.reload }}
        >
          The usage for this month could not be read: the answer was incomplete.
        </InlineStatus>
      ) : null}
      {read.status === "unavailable" ? (
        <InlineStatus role="status">This deployment does not report usage.</InlineStatus>
      ) : null}
      {read.status === "signed-out" ? (
        <InlineStatus
          role="status"
          action={{
            label: "Sign in",
            // a deployment with no sign-in built in has nothing to open: reading again is all there is
            onClick: () => {
              if (openSignIn() === "none") read.reload();
            },
          }}
        >
          You are signed out. Sign in to see your usage.
        </InlineStatus>
      ) : null}
      {read.status === "error" ? (
        <InlineStatus
          tone="error"
          role="alert"
          action={{ label: "Try again", onClick: read.reload }}
        >
          {read.message}
        </InlineStatus>
      ) : null}
    </main>
  );
}
