// @vitest-environment jsdom
import { AssistantRuntimeProvider } from "@assistant-ui/react";
import { type AgUiAssistantRuntime, useAgUiRuntime } from "@assistant-ui/react-ag-ui";
import { act, cleanup, configure, render, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import type { EarlierControl } from "@/features/chat/components/earlier";
import { HistorySeed } from "@/features/chat/components/history-seed";
import { LiveRuns } from "@/features/chat/components/live-runs";
import {
  fakeFetch,
  framesThrough,
  type GoldenFrame,
  LiveStream,
  loadGolden,
  sse,
  THREAD_ID,
} from "@/features/chat/lib/agui/testing";
import { summarize } from "@/features/chat/lib/agui/testing-runtime";
import { ThreadAgent } from "@/features/chat/lib/agui/thread-agent";
import { useEarlier } from "./use-earlier";

configure({ asyncUtilTimeout: 10_000 });
afterEach(cleanup);

/**
 * Older turns put in front of a transcript that is on screen (ADR 0059), with the real runtime: what the import must not
 * disturb. A question the agent asked in the newest turn is still waiting for the person afterwards (the transcript that is
 * imported holds its message, and a pending question is read off the last message). That the older turns are what a replay
 * shows is `windowed.dom.test.tsx`.
 */

const CONFIG = { initialTurns: 12, pageTurns: 20, maxTurns: 100 };

/** The frames with the ids of their messages and runs spelled differently, so two goldens can be one thread. */
const renamed = (frames: GoldenFrame[], prefix: string): GoldenFrame[] =>
  JSON.parse(
    JSON.stringify(frames)
      .replaceAll('"evt-', `"${prefix}evt-`)
      .replaceAll('"run-', `"${prefix}run-`),
  );

/** Their resume points moved up by `by`, as a later part of the log has them. */
const shifted = (frames: GoldenFrame[], by: number): GoldenFrame[] =>
  frames.map((f) => (f.id === undefined ? f : { ...f, id: f.id + by }));

// the older turn: a whole turn of `followup`; the newest: a turn of `ask` that ends on a question nobody has answered
const OLDER = renamed(framesThrough(loadGolden("followup"), 5), "old-");
const NEWEST = shifted(framesThrough(loadGolden("ask"), 4), 5);

const page = (start: number, end: number, earlier: boolean, frames: GoldenFrame[]) =>
  new Response(JSON.stringify({ start, end, head: 9, earlier, projection: 1, frames }), {
    headers: { "content-type": "application/json" },
  });

function mount() {
  const connect = new LiveStream();
  const { fetch, calls } = fakeFetch((call, request) => {
    if (!call.path.endsWith("/history")) return sse(connect.body);
    return new URL(request.url).searchParams.has("before")
      ? page(1, 5, false, OLDER)
      : page(6, 9, true, NEWEST);
  });
  const agent = new ThreadAgent({
    threadId: THREAD_ID,
    fetch,
    baseUrl: "http://orch.test",
    target: () => ({ agentId: "plain", release: null }),
    backoff: () => 1,
    history: () => CONFIG,
  });
  const box: { runtime?: AgUiAssistantRuntime; earlier?: EarlierControl } = {};
  function Harness() {
    const runtime = useAgUiRuntime({ agent, resumeTranscript: "appended" });
    box.runtime = runtime;
    box.earlier = useEarlier(agent, runtime);
    return (
      <AssistantRuntimeProvider runtime={runtime}>
        <LiveRuns agent={agent} runtime={runtime} />
        <HistorySeed agent={agent} runtime={runtime} />
      </AssistantRuntimeProvider>
    );
  }
  render(<Harness />);
  const runtime = () => box.runtime as AgUiAssistantRuntime;
  return {
    agent,
    calls,
    connect,
    runtime,
    earlier: () => box.earlier as EarlierControl,
    messages: () => runtime().thread.getState().messages,
  };
}

describe("older turns put in front of the transcript", () => {
  it("leave the question of the newest turn waiting for the person", async () => {
    const m = mount();
    m.agent.start();
    await waitFor(() => expect(m.messages()).toHaveLength(2));
    await waitFor(() => expect(m.agent.idleForImport()).toBe(true));
    // a question the person has not answered: the form of the newest turn
    expect(m.runtime().unstable_getPendingInterrupts()).toMatchObject([{ id: "int-3" }]);
    expect(m.earlier().earlier).toBe(true);

    await act(async () => {
      m.earlier().load();
    });
    await waitFor(() => expect(m.messages()).toHaveLength(4));
    await waitFor(() => expect(m.earlier().state).toBe("idle"));

    // the question is as it was: still the last thing, still to be answered
    expect(m.runtime().unstable_getPendingInterrupts()).toMatchObject([{ id: "int-3" }]);
    expect(summarize(m.messages()).map((x) => x.role)).toEqual([
      "user",
      "assistant",
      "user",
      "assistant",
    ]);
    // and it is the newest turn's alone: the older turn is a turn that ended
    expect(summarize(m.messages()).map((x) => x.status)).toEqual([
      undefined,
      "complete",
      undefined,
      "requires-action:interrupt",
    ]);
    expect(m.earlier().earlier).toBe(false);
    m.agent.stop();
  });
});
