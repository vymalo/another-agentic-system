// @vitest-environment jsdom
import { AssistantRuntimeProvider } from "@assistant-ui/react";
import { type AgUiAssistantRuntime, useAgUiRuntime } from "@assistant-ui/react-ag-ui";
import { act, cleanup, configure, render, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
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
import { LOAD_ALL_PAGES, useEarlier } from "./use-earlier";

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

const page = (start: number, end: number, earlier: boolean, frames: GoldenFrame[], head = 9) =>
  new Response(JSON.stringify({ start, end, head, earlier, projection: 1, frames }), {
    headers: { "content-type": "application/json" },
  });

function mount() {
  const connect = new LiveStream();
  const { fetch, calls } = fakeFetch((call, request) => {
    if (call.method === "POST") {
      // a message or an answer is accepted at once: the run it starts comes by the connect stream
      const accepted = new LiveStream();
      accepted.frames([
        {
          event: {
            type: "RUN_STARTED",
            threadId: THREAD_ID,
            runId: (call.body as { runId: string }).runId,
            protocolVersion: "1.0",
          },
        },
      ]);
      return sse(accepted.body);
    }
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

  it("leave the question answerable: the answer is sent as the resume of its interrupt, once", async () => {
    const m = mount();
    m.agent.start();
    await waitFor(() => expect(m.messages()).toHaveLength(2));
    await waitFor(() => expect(m.agent.idleForImport()).toBe(true));
    await act(async () => {
      m.earlier().load();
    });
    await waitFor(() => expect(m.messages()).toHaveLength(4));
    await waitFor(() => expect(m.earlier().state).toBe("idle"));
    expect(m.runtime().unstable_getPendingInterrupts()).toMatchObject([{ id: "int-3" }]);

    // the answer's run is the agent's to finish; what this asks is what was sent
    void m
      .runtime()
      .unstable_submitInterruptResponses([
        { interruptId: "int-3", status: "resolved", payload: { text: "main" } },
      ])
      .catch(() => {});
    await waitFor(() => expect(m.calls.some((c) => c.method === "POST")).toBe(true));
    const posts = m.calls.filter((c) => c.method === "POST");
    expect(posts).toHaveLength(1);
    const body = posts[0]?.body as Record<string, unknown>;
    expect(body.messages).toEqual([]);
    expect(body.resume).toEqual([
      { interruptId: "int-3", status: "resolved", payload: { text: "main" } },
    ]);
    m.agent.stop();
  });
});

/** A turn of the thread that begins on the stream after the page: its frames are numbered past the page's end. */
const LIVE = shifted(renamed(loadGolden("followup").slice(0, 13), "live-"), 100);

describe("a page that cannot be put in front of the transcript yet, or at all", () => {
  it("is not held while it waits: the labels, the pages and the carry stay as they were until the import is made", async () => {
    const m = mount();
    m.agent.start();
    await waitFor(() => expect(m.messages()).toHaveLength(2));
    await waitFor(() => expect(m.agent.idleForImport()).toBe(true));
    // a run is open: the import has to wait for it
    await act(async () => {
      m.connect.frames(LIVE.slice(0, 5));
    });
    await waitFor(() => expect(m.agent.idleForImport()).toBe(false));
    const was = m.agent.getHistory();
    const usage = m.agent.getSnapshot().usage;

    await act(async () => {
      m.earlier().load();
    });
    await waitFor(() => expect(m.earlier().state).toBe("waiting"));
    // the page is read (the account of the pages has not taken it) and nothing the screen shows has changed
    expect(m.calls.filter((c) => c.path.endsWith("/history") && c.method === "GET")).toHaveLength(
      2,
    );
    expect(m.agent.getHistory()).toBe(was);
    expect(m.agent.getHistory().earlier).toBe(true);
    expect(m.agent.getSnapshot().usage).toEqual(usage);

    // the run ends and is taken by the runtime: now the page goes in, and only now it is held
    await act(async () => {
      m.connect.frames(LIVE.slice(5));
    });
    await waitFor(() => expect(m.earlier().state).toBe("idle"));
    expect(m.agent.getHistory().earlier).toBe(false);
    expect(summarize(m.messages())[0]?.role).toBe("user");
    expect(m.messages().length).toBeGreaterThanOrEqual(4);
    m.agent.stop();
  });

  it("is asked for again when its import fails: nothing was held, and Retry reads the same page and puts it in", async () => {
    const m = mount();
    m.agent.start();
    await waitFor(() => expect(m.messages()).toHaveLength(2));
    await waitFor(() => expect(m.agent.idleForImport()).toBe(true));
    const was = m.agent.getHistory();
    const thread = m.runtime().thread;
    const real = thread.import.bind(thread);
    const failing = vi.spyOn(thread, "import").mockImplementationOnce(() => {
      throw new Error("the transcript could not be replaced");
    });

    await act(async () => {
      m.earlier().load();
    });
    await waitFor(() => expect(m.earlier().state).toBe("error"));
    expect(m.earlier().error).toBe("the transcript could not be replaced");
    expect(m.agent.getHistory()).toBe(was);
    expect(m.earlier().earlier).toBe(true);
    expect(m.messages()).toHaveLength(2);

    // the runs are let go again, and the same page is read for the retry
    failing.mockImplementation(real);
    await act(async () => {
      m.earlier().load();
    });
    await waitFor(() => expect(m.earlier().state).toBe("idle"));
    await waitFor(() => expect(m.messages()).toHaveLength(4));
    expect(m.earlier().earlier).toBe(false);
    expect(m.earlier().error).toBeNull();
    const asked = m.calls.filter((c) => c.path.endsWith("/history"));
    expect(asked).toHaveLength(3);
    m.agent.stop();
  });

  it("looks at the moment again once the runs are held, and lets them go when it was not one", async () => {
    const m = mount();
    m.agent.start();
    await waitFor(() => expect(m.messages()).toHaveLength(2));
    await waitFor(() => expect(m.agent.idleForImport()).toBe(true));
    // something begins between the look and the hold: the agent is not idle until the first hold is let go
    const pause = m.agent.pauseRuns.bind(m.agent);
    const idle = m.agent.idleForImport.bind(m.agent);
    let holds = 0;
    let firstLetGo = false;
    vi.spyOn(m.agent, "pauseRuns").mockImplementation(() => {
      const mine = ++holds;
      const release = pause();
      return () => {
        if (mine === 1) firstLetGo = true;
        release();
      };
    });
    vi.spyOn(m.agent, "idleForImport").mockImplementation(() =>
      holds === 1 && !firstLetGo ? false : idle(),
    );

    await act(async () => {
      m.earlier().load();
    });
    await waitFor(() => expect(m.earlier().state).toBe("idle"));
    await waitFor(() => expect(m.messages()).toHaveLength(4));
    expect(holds).toBe(2);
    m.agent.stop();
  });
});

describe("loading all the older turns", () => {
  /** A thread of `pages` older pages of five events and a turn each, behind the newest page: events `5 * pages + 1` to `5 * pages + 4`. */
  function mountMany(pages: number) {
    const connect = new LiveStream();
    const first = 5 * pages + 1;
    const { fetch, calls } = fakeFetch((call, request) => {
      if (!call.path.endsWith("/history")) return sse(connect.body);
      const before = new URL(request.url).searchParams.get("before");
      if (before === null)
        return page(first, first + 3, true, shifted(NEWEST, first - 6), first + 3);
      const end = Number(before) - 1;
      const start = end - 4;
      return page(start, end, start > 1, shifted(renamed(OLDER, `e${end}-`), start - 1), first + 3);
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
    return {
      agent,
      calls,
      earlier: () => box.earlier as EarlierControl,
      messages: () => (box.runtime as AgUiAssistantRuntime).thread.getState().messages,
    };
  }

  it("stops after a few pages and leaves the rest to the next click", async () => {
    const m = mountMany(LOAD_ALL_PAGES + 3);
    m.agent.start();
    await waitFor(() => expect(m.messages()).toHaveLength(2));
    await waitFor(() => expect(m.agent.idleForImport()).toBe(true));
    const older = () => m.calls.filter((c) => c.path.endsWith("/history")).length - 1;

    await act(async () => {
      m.earlier().loadAll();
    });
    await waitFor(() => expect(m.earlier().state).toBe("idle"));
    // the cap: the older pages read are those of one click, and more are left
    expect(older()).toBe(LOAD_ALL_PAGES);
    expect(m.earlier().earlier).toBe(true);
    await waitFor(() => expect(m.messages()).toHaveLength(2 + 2 * LOAD_ALL_PAGES));

    await act(async () => {
      m.earlier().loadAll();
    });
    await waitFor(() => expect(m.earlier().state).toBe("idle"));
    await waitFor(() => expect(m.earlier().earlier).toBe(false));
    expect(older()).toBe(LOAD_ALL_PAGES + 3);
    await waitFor(() => expect(m.messages()).toHaveLength(2 + 2 * (LOAD_ALL_PAGES + 3)));
    m.agent.stop();
  });
});
