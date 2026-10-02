"use client";

import type { ReactNode } from "react";
import { inputCut, type StepInput, type StepOutput } from "@/features/chat/lib/agui/vymalo";
import { formatBytes, notKept } from "@/features/chat/lib/step-label";
import { cn } from "@/lib/utils";

/*
 * What a tool step opens onto: what it was called with, what it returned, and the error when it
 * failed (ADR 0030, web/DESIGN.md "Steps panel"). All of it is an agent's text, cut and redacted by
 * the orchestrator and drawn here as text nodes: nothing is parsed, nothing is markup.
 */

type Scalar = string | number | boolean | null;
const isScalar = (v: unknown): v is Scalar =>
  v === null || ["string", "number", "boolean"].includes(typeof v);

/** An input whose values are all plain (no object, no list) reads as a list of `key  value`. */
const isFlat = (input: StepInput): boolean => Object.values(input).every(isScalar);

const BOX =
  "block max-h-64 max-w-full overflow-auto rounded-md border bg-muted/60 px-2 py-1 font-mono text-xs leading-5 whitespace-pre-wrap text-foreground/90 [overflow-wrap:anywhere] focus-visible:ring-3 focus-visible:ring-ring/50 focus-visible:outline-none";

function Part({
  id,
  title,
  tone,
  children,
}: {
  id: string;
  title: string;
  tone?: "error";
  children: ReactNode;
}) {
  return (
    <div data-slot="step-io-part" className="flex min-w-0 flex-col gap-1">
      <h4
        id={id}
        className={cn(
          "text-xs font-medium",
          tone === "error" ? "text-destructive" : "text-muted-foreground",
        )}
      >
        {title}
      </h4>
      {children}
    </div>
  );
}

function InputView({ input }: { input: StepInput }) {
  const cut = inputCut(input);
  if (cut) {
    return (
      <p data-slot="step-input-cut" className="text-xs text-muted-foreground">
        Input not kept ({formatBytes(cut.bytes)})
      </p>
    );
  }
  if (isFlat(input)) {
    return (
      <dl
        data-slot="step-input"
        className="grid min-w-0 grid-cols-[minmax(0,auto)_minmax(0,1fr)] gap-x-3 gap-y-1 text-xs leading-5"
      >
        {Object.entries(input).map(([key, value]) => (
          <div key={key} className="col-span-2 grid grid-cols-subgrid">
            <dt className="font-mono text-muted-foreground [overflow-wrap:anywhere]">{key}</dt>
            <dd className="m-0 whitespace-pre-wrap text-foreground/90 [overflow-wrap:anywhere]">
              {value === null ? "null" : String(value)}
            </dd>
          </div>
        ))}
      </dl>
    );
  }
  return (
    // biome-ignore lint/a11y/noNoninteractiveTabindex: a scroll box has to be reachable to be scrolled by keyboard
    <pre data-slot="step-input" tabIndex={0} className={BOX}>
      {JSON.stringify(input, null, 2)}
    </pre>
  );
}

function OutputView({ output, text }: { output: StepOutput; text: string }) {
  const missing = notKept(output);
  return (
    <>
      {text === "" ? (
        <p className="text-xs text-muted-foreground">Nothing was returned.</p>
      ) : (
        // biome-ignore lint/a11y/noNoninteractiveTabindex: a scroll box has to be reachable to be scrolled by keyboard
        <pre data-slot="step-output" tabIndex={0} className={BOX}>
          {text}
        </pre>
      )}
      {output.truncated ? (
        <p data-slot="step-output-cut" className="text-xs text-muted-foreground">
          {missing !== undefined && missing > 0
            ? `${formatBytes(missing)} more not kept`
            : "More was not kept"}
        </p>
      ) : null}
    </>
  );
}

/**
 * The block of a tool step: Input, then Output, or Error in its place when the call failed, and a
 * note when the job's record limit left something out. `error` is what a failed step says when its
 * output is not marked: the output's own text, else the step's detail.
 */
export function StepIo({
  id,
  input,
  output,
  ioDropped,
  failed,
  detail,
}: {
  id: string;
  input?: StepInput | undefined;
  output?: StepOutput | undefined;
  ioDropped?: boolean | undefined;
  failed: boolean;
  detail?: string | undefined;
}) {
  const isError = output?.error === true || failed;
  const errorText = output ? output.text : failed ? detail : undefined;
  return (
    <div
      id={id}
      data-slot="step-io"
      className="flex min-w-0 flex-col gap-2 rounded-lg border bg-card px-2.5 py-2"
    >
      {input ? (
        <Part id={`${id}-in`} title="Input">
          <InputView input={input} />
        </Part>
      ) : null}
      {output && !isError ? (
        <Part id={`${id}-out`} title="Output">
          <OutputView output={output} text={output.text} />
        </Part>
      ) : null}
      {isError && errorText !== undefined ? (
        <Part id={`${id}-err`} title="Error" tone="error">
          {output ? (
            <OutputView output={output} text={errorText} />
          ) : (
            <p className="text-xs whitespace-pre-wrap [overflow-wrap:anywhere]">{errorText}</p>
          )}
        </Part>
      ) : null}
      {ioDropped ? (
        <p data-slot="step-io-dropped" className="text-xs text-muted-foreground">
          Some of this step&apos;s input or output was not kept: the job passed its recording limit.
        </p>
      ) : null}
      {!input && !output && !ioDropped ? (
        <p className="text-xs text-muted-foreground">No details.</p>
      ) : null}
    </div>
  );
}
