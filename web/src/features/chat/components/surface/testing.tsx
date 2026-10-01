import type { ReactNode } from "react";
import { Thread } from "@/components/assistant-ui/elements/thread.aui";
import { TooltipProvider } from "@/components/ui/tooltip";
import { DataUIs } from "@/features/chat/components/data-uis";
import { type ThreadView, ThreadViewProvider } from "@/features/chat/components/thread-view";
import { type GoldenFrame, LiveStream, sse, THREAD_ID } from "@/features/chat/lib/agui/testing";
import { mountRuntime } from "@/features/chat/lib/agui/testing-runtime";
import type { ThreadAgentOptions } from "@/features/chat/lib/agui/thread-agent";
import { type SurfaceHost, SurfaceHostContext } from "./surface-host";

/**
 * Test support for the surface renderer: the real runtime, the real transcript and the real
 * renderers (DataUIs), a host the test controls, and streams the test writes. jsdom only.
 */

export const HOST: SurfaceHost = {
  state: "blocked",
  canSend: true,
  canCompose: true,
  send: () => {},
  fillComposer: () => {},
  reject: () => {},
};

let seq = 0;
const frame = (event: Record<string, unknown>, id?: number): GoldenFrame => ({
  event,
  ...(id !== undefined ? { id } : {}),
});
const agent = { "vymalo.actor": { type: "agent", name: "plain" } };

/**
 * One run of the AG-UI story of a surface: the snapshots are `ACTIVITY_SNAPSHOT`s of ONE surface
 * message (as the orchestrator sends them, `replace: true`), and the run ends as `end` says.
 */
export function surfaceRun(
  snapshots: unknown[][],
  {
    runId = "run-1",
    messageId = "a2ui-3",
    end = "interrupt",
    user = true,
    metadata = agent,
  }: {
    runId?: string;
    messageId?: string;
    end?: "interrupt" | "success" | "error";
    user?: boolean;
    metadata?: Record<string, unknown>;
  } = {},
): GoldenFrame[] {
  const first = seq + 1;
  const out: GoldenFrame[] = [
    frame({ type: "RUN_STARTED", threadId: THREAD_ID, runId, protocolVersion: "1.0" }),
  ];
  if (user) {
    out.push(
      frame({ type: "TEXT_MESSAGE_START", messageId: `evt-${first}`, role: "user" }),
      frame({ type: "TEXT_MESSAGE_CONTENT", messageId: `evt-${first}`, delta: "ui" }),
      frame({ type: "TEXT_MESSAGE_END", messageId: `evt-${first}` }, ++seq),
    );
  }
  out.push(
    frame({ type: "SUBAGENT_STARTED", name: "plain", subagentRunId: "sub-1", metadata: agent }),
  );
  for (const operations of snapshots) {
    out.push(
      frame(
        {
          type: "ACTIVITY_SNAPSHOT",
          messageId,
          activityType: "a2ui-surface",
          replace: true,
          content: { a2ui_operations: operations },
          subagentRunId: "sub-1",
          metadata,
        },
        ++seq,
      ),
    );
  }
  if (end === "interrupt") {
    out.push(
      frame({
        type: "SUBAGENT_FINISHED",
        subagentRunId: "sub-1",
        outcome: { type: "suspended", interruptIds: ["int-1"] },
      }),
      frame({
        type: "STATE_SNAPSHOT",
        snapshot: { thread: { state: "blocked", target: { agentId: "plain" }, title: "ui" } },
      }),
      frame(
        {
          type: "RUN_FINISHED",
          threadId: THREAD_ID,
          runId,
          outcome: {
            type: "interrupt",
            interrupts: [
              {
                id: "int-1",
                reason: "input_required",
                message: "Pick one",
                subagentRunId: "sub-1",
              },
            ],
          },
        },
        ++seq,
      ),
    );
  } else if (end === "error") {
    out.push(
      frame({
        type: "SUBAGENT_ERROR",
        subagentRunId: "sub-1",
        message: "gone",
        code: "delivery_failed",
      }),
      frame({
        type: "STATE_SNAPSHOT",
        snapshot: { thread: { state: "blocked", target: { agentId: "plain" }, title: "ui" } },
      }),
      frame({ type: "RUN_ERROR", message: "gone", code: "delivery_failed" }, ++seq),
    );
  } else {
    out.push(
      frame({ type: "SUBAGENT_FINISHED", subagentRunId: "sub-1" }),
      frame(
        { type: "RUN_FINISHED", threadId: THREAD_ID, runId, outcome: { type: "success" } },
        ++seq,
      ),
    );
  }
  return out;
}

/**
 * The next run of the story, started by the person's action (`vymalo.action`, then the agent's
 * words): what a thread shows after a Choices was answered. `content` is the action's content,
 * as the orchestrator projects it (`surfaceId`, `name`, `sourceComponentId`, `context`).
 */
export function actionRun(
  content: Record<string, unknown>,
  { runId = "run-2", words = "Going on." }: { runId?: string; words?: string } = {},
): GoldenFrame[] {
  const id = ++seq;
  const user = { "vymalo.actor": { type: "user", name: "alice@example.com" } };
  const text = `st-${id}`;
  return [
    frame({ type: "RUN_STARTED", threadId: THREAD_ID, runId, protocolVersion: "1.0" }),
    frame({
      type: "STATE_SNAPSHOT",
      snapshot: { thread: { state: "queued", target: { agentId: "plain" }, title: "ui" } },
    }),
    frame(
      {
        type: "ACTIVITY_SNAPSHOT",
        messageId: `evt-${id}`,
        activityType: "vymalo.action",
        replace: false,
        content: { at: "2027-01-15T08:00:07Z", ...content },
        metadata: user,
      },
      id,
    ),
    frame({ type: "SUBAGENT_STARTED", name: "plain", subagentRunId: "sub-2", metadata: agent }),
    frame({
      type: "TEXT_MESSAGE_START",
      messageId: text,
      role: "assistant",
      subagentRunId: "sub-2",
      metadata: agent,
    }),
    frame({ type: "TEXT_MESSAGE_CONTENT", messageId: text, delta: words, subagentRunId: "sub-2" }),
    frame({ type: "TEXT_MESSAGE_END", messageId: text, subagentRunId: "sub-2" }),
    frame({ type: "SUBAGENT_FINISHED", subagentRunId: "sub-2" }, ++seq),
    frame({
      type: "STATE_SNAPSHOT",
      snapshot: { thread: { state: "done", target: { agentId: "plain" }, title: "ui" } },
    }),
    frame(
      { type: "RUN_FINISHED", threadId: THREAD_ID, runId, outcome: { type: "success" } },
      ++seq,
    ),
  ];
}

export const resetSeq = () => {
  seq = 0;
};

/**
 * The runtime, the transcript and the renderers over a stream the test writes to, with `host`
 * as the app's side of a surface (what a click does).
 */
export function mountSurfaces(
  host: Partial<SurfaceHost> = {},
  options: Partial<ThreadAgentOptions> = {},
  extra?: ReactNode,
  /** What the transcript knows of the thread: its UI catalog version, say. */
  view: Partial<ThreadView> = {},
) {
  const stream = new LiveStream();
  const value: SurfaceHost = { ...HOST, ...host };
  const threadView: ThreadView = { state: "blocked", waiting: true, agentId: "plain", ...view };
  const mounted = mountRuntime(
    () => sse(stream.body),
    options,
    <SurfaceHostContext.Provider value={value}>
      <ThreadViewProvider value={threadView}>
        <TooltipProvider>
          <DataUIs />
          <Thread loading={false} empty={false} />
          {extra}
        </TooltipProvider>
      </ThreadViewProvider>
    </SurfaceHostContext.Provider>,
  );
  mounted.agent.start();
  return { ...mounted, stream };
}

/** The viewport needs these to exist: jsdom has no layout. */
export function stubLayout() {
  globalThis.ResizeObserver = class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
  Element.prototype.scrollTo = () => {};
  Element.prototype.scrollIntoView = () => {};
}
