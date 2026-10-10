// @vitest-environment jsdom
import { act, cleanup, configure, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { HistoryGap, ProjectionChanged } from "./history-window";
import {
  type Call,
  fakeFetch,
  type GoldenFrame,
  LiveStream,
  loadGolden,
  problem,
  sse,
  THREAD_ID,
} from "./testing";
import { ThreadAgent, type ThreadAgentOptions } from "./thread-agent";

configure({ asyncUtilTimeout: 5_000 });
afterEach(cleanup);

/**
 * A thread opened at its end (ADR 0059, docs/api/history.md): the newest page of history is read instead of the connect
 * stream's replay, its runs are the seed, the stream follows from the page's `end`, and older pages come on request.
 *
 * The `followup` golden is two turns; the pages are the ones the real fold writes for it (docs/api/examples/history/followup.pages.json).
 */

const CONFIG = { initialTurns: 12, pageTurns: 20, maxTurns: 100 };
const golden = loadGolden("followup");
const NEWEST = {
  start: 6,
  end: 11,
  head: 11,
  earlier: true,
  projection: 1,
  frames: golden.slice(13),
};
const OLDEST = {
  start: 1,
  end: 5,
  head: 11,
  earlier: false,
  projection: 1,
  frames: golden.slice(0, 13),
};

type Page = typeof NEWEST;
const json = (body: unknown) =>
  new Response(JSON.stringify(body), { headers: { "content-type": "application/json" } });
const queryOf = (request: Request) => Object.fromEntries(new URL(request.url).searchParams);
const isHistory = (call: Call) => call.path === `/agui/threads/${THREAD_ID}/history`;

function open(
  answer: (call: Call, request: Request) => Response | Promise<Response>,
  options: Partial<ThreadAgentOptions> = {},
  /** What the orchestrator says to a message sent (the connect stream is the default). */
  post?: (call: Call) => Response | Promise<Response>,
) {
  const connect = new LiveStream();
  const { fetch, calls } = fakeFetch((call, request) =>
    isHistory(call)
      ? answer(call, request)
      : call.method === "POST" && post
        ? post(call)
        : sse(connect.body),
  );
  const agent = new ThreadAgent({
    threadId: THREAD_ID,
    fetch,
    baseUrl: "http://orch.test",
    target: () => ({ agentId: "plain", release: null }),
    backoff: () => 1,
    history: () => CONFIG,
    ...options,
  });
  return { agent, calls, connect };
}

const connects = (calls: Call[]) => calls.filter((c) => c.method === "GET" && !isHistory(c));

describe("a thread opened at its end", () => {
  it("reads the newest page first, holds the stream until the seed is in, then follows from the page's end", async () => {
    const asked: Record<string, string>[] = [];
    const { agent, calls } = open((_call, request) => {
      asked.push(queryOf(request));
      return json(NEWEST);
    });
    agent.start();
    await waitFor(() => expect(agent.getHistory().enabled).toBe(true));
    expect(asked).toEqual([{ limit: "12" }]);
    expect(agent.getHistory()).toMatchObject({ enabled: true, earlier: true });
    // the transcript is held back, and no connect has gone out: the stream waits for the seed
    expect(agent.getSnapshot().replaying).toBe(true);
    expect(agent.getSnapshot().lastSeq).toBe(11);
    expect(connects(calls)).toHaveLength(0);

    const runs = agent.takeSeed();
    expect(runs).toHaveLength(1);
    expect(runs?.[0]?.userMessages).toHaveLength(1);
    // taken once
    expect(agent.takeSeed()).toBeNull();

    agent.seeded();
    await waitFor(() => expect(connects(calls)).toHaveLength(1));
    expect(connects(calls)[0]?.lastEventId).toBe("11");
    await waitFor(() => expect(agent.getSnapshot().replaying).toBe(false));
    agent.stop();
  });

  it("a thread with nothing to show opens with no seed and follows the stream from its end", async () => {
    const { agent, calls } = open(() =>
      json({ start: 1, end: 0, head: 0, earlier: false, projection: 1, frames: [] }),
    );
    agent.start();
    await waitFor(() => expect(connects(calls)).toHaveLength(1));
    expect(agent.takeSeed()).toBeNull();
    expect(agent.getHistory()).toMatchObject({ enabled: true, earlier: false });
    agent.stop();
  });

  it("reads the link's route for a shared thread", async () => {
    const paths: string[] = [];
    const connect = new LiveStream();
    const { fetch } = fakeFetch((call) => {
      paths.push(call.path);
      return call.path.endsWith("/history")
        ? json({ ...NEWEST, earlier: false })
        : sse(connect.body);
    });
    const agent = new ThreadAgent({
      threadId: THREAD_ID,
      fetch,
      baseUrl: "http://orch.test",
      target: () => ({ agentId: "plain", release: null }),
      backoff: () => 1,
      history: () => CONFIG,
      source: { audience: "public", token: "tok" },
    });
    agent.start();
    await waitFor(() => expect(agent.takeSeed()).not.toBeNull());
    expect(paths).toContain("/agui/public/shared/tok/history");
    agent.stop();
  });

  it("a history route that is not there opens the thread the old way, and the stream says whether the thread is", async () => {
    // an orchestrator that does not serve the route answers 404 for a thread that exists
    const { agent, calls } = open(() => problem(404, "Not Found"));
    agent.start();
    await waitFor(() => expect(connects(calls)).toHaveLength(1));
    expect(connects(calls)[0]?.lastEventId).toBeUndefined();
    expect(agent.getHistory().enabled).toBe(false);
    expect(agent.getSnapshot().notFound).toBe(false);
    agent.stop();
  });

  it("a thread that is not there is not found: the replay's stream is the 404 that counts", async () => {
    const fetched = fakeFetch(() => problem(404, "Not Found"));
    const agent = new ThreadAgent({
      threadId: THREAD_ID,
      fetch: fetched.fetch,
      baseUrl: "http://orch.test",
      target: () => ({ agentId: "plain", release: null }),
      backoff: () => 1,
      history: () => CONFIG,
    });
    agent.start();
    await waitFor(() => expect(agent.getSnapshot().notFound).toBe(true));
    agent.stop();
  });

  it("a page that cannot be read opens the thread the old way, from its first event", async () => {
    const { agent, calls } = open(() => problem(500, "Boom"));
    agent.start();
    await waitFor(() => expect(connects(calls)).toHaveLength(1));
    expect(connects(calls)[0]?.lastEventId).toBeUndefined();
    expect(agent.getHistory().enabled).toBe(false);
    expect(agent.takeSeed()).toBeNull();
    agent.stop();
  });

  it("a seed that could not be made is opened the old way too, from the first event", async () => {
    const { agent, calls } = open(() => json(NEWEST));
    agent.start();
    await waitFor(() => expect(agent.takeSeed()).not.toBeNull());
    agent.seedFailed(new Error("no"));
    await waitFor(() => expect(connects(calls)).toHaveLength(1));
    expect(connects(calls)[0]?.lastEventId).toBeUndefined();
    expect(agent.getHistory().enabled).toBe(false);
    agent.stop();
  });

  it("without the capability the history route is never asked for", async () => {
    const { agent, calls } = open(() => json(NEWEST), { history: () => undefined });
    agent.start();
    await waitFor(() => expect(connects(calls)).toHaveLength(1));
    expect(calls.some(isHistory)).toBe(false);
    expect(agent.getHistory().enabled).toBe(false);
    agent.stop();
  });
});

describe("a link to a message (#m-<seq>)", () => {
  it("asks for the turns back to it, instead of the newest few", async () => {
    const asked: Record<string, string>[] = [];
    const { agent } = open(
      (_call, request) => {
        asked.push(queryOf(request));
        return json({ ...NEWEST, start: 1, earlier: false });
      },
      { anchor: () => 3 },
    );
    agent.start();
    await waitFor(() => expect(agent.takeSeed()).not.toBeNull());
    expect(asked).toEqual([{ since: "3" }]);
    expect(agent.getHistory().anchorMissed).toBe(false);
    agent.stop();
  });

  it("says so when the page cannot reach the message, and shows the end of the thread", async () => {
    const { agent } = open(() => json(NEWEST), { anchor: () => 3 });
    agent.start();
    await waitFor(() => expect(agent.takeSeed()).not.toBeNull());
    // the page starts at event 6
    expect(agent.getHistory()).toMatchObject({ enabled: true, anchorMissed: true });
    agent.stop();
  });

  it("opens at the end, with nothing said, when the log has no such event", async () => {
    const asked: Record<string, string>[] = [];
    const { agent } = open(
      (_call, request) => {
        const query = queryOf(request);
        asked.push(query);
        return json(
          query.since
            ? { start: 12, end: 11, head: 11, earlier: true, projection: 1, frames: [] }
            : NEWEST,
        );
      },
      { anchor: () => 900 },
    );
    agent.start();
    await waitFor(() => expect(agent.takeSeed()).not.toBeNull());
    expect(asked).toEqual([{ since: "900" }, { limit: "12" }]);
    expect(agent.getHistory().anchorMissed).toBe(false);
    agent.stop();
  });

  it("is not missed any more once an older page is held", async () => {
    const { agent } = open((_call, request) => json(queryOf(request).before ? OLDEST : NEWEST), {
      anchor: () => 3,
    });
    agent.start();
    await waitFor(() => expect(agent.takeSeed()).not.toBeNull());
    agent.seeded();
    expect(agent.getHistory().anchorMissed).toBe(true);
    await agent.fetchEarlier();
    expect(agent.getHistory().anchorMissed).toBe(false);
    agent.stop();
  });
});

describe("older pages", () => {
  async function opened(answer: (query: Record<string, string>) => Page | Response) {
    const asked: Record<string, string>[] = [];
    const rig = open((_call, request) => {
      const query = queryOf(request);
      asked.push(query);
      const got = answer(query);
      return got instanceof Response ? got : json(got);
    });
    rig.agent.start();
    await waitFor(() => expect(rig.agent.takeSeed()).not.toBeNull());
    rig.agent.seeded();
    await waitFor(() => expect(connects(rig.calls)).toHaveLength(1));
    return { ...rig, asked };
  }

  it("asks for the pages before the oldest one held, and says when there are no more", async () => {
    const { agent, asked } = await opened((q) => (q.before ? OLDEST : NEWEST));
    const runs = await agent.fetchEarlier();
    expect(runs).toHaveLength(1);
    expect(asked.at(-1)).toEqual({ limit: "20", before: "6" });
    expect(agent.getHistory()).toMatchObject({ enabled: true, earlier: false });
    // nothing older: no more asks
    expect(await agent.fetchEarlier()).toEqual([]);
    expect(asked).toHaveLength(2);
    agent.stop();
  });

  it("one read at a time: a second call is the first one's answer", async () => {
    const { agent, asked } = await opened((q) => (q.before ? OLDEST : NEWEST));
    const [a, b] = await Promise.all([agent.fetchEarlier(), agent.fetchEarlier()]);
    expect(a).toBe(b);
    expect(asked.filter((q) => q.before)).toHaveLength(1);
    agent.stop();
  });

  it("a page that does not join the ones held is refused and the account is as it was", async () => {
    const gap: Page = { ...OLDEST, end: 3, frames: OLDEST.frames as GoldenFrame[] } as Page;
    let first = true;
    const { agent, asked } = await opened((q) => {
      if (!q.before) return NEWEST;
      if (first) {
        first = false;
        return gap;
      }
      return OLDEST;
    });
    await expect(agent.fetchEarlier()).rejects.toBeInstanceOf(HistoryGap);
    expect(agent.getHistory().earlier).toBe(true);
    // asking again reads the right one
    expect(await agent.fetchEarlier()).toHaveLength(1);
    expect(agent.getHistory().earlier).toBe(false);
    expect(asked.filter((q) => q.before)).toEqual([
      { limit: "20", before: "6" },
      { limit: "20", before: "6" },
    ]);
    agent.stop();
  });

  it("holds a page only when told to: until then the account is as it was, and a second read is the same page again", async () => {
    const { agent, asked } = await opened((q) => (q.before ? OLDEST : NEWEST));
    const view = agent.getHistory();
    const read = await agent.readEarlier();
    expect(read.runs).toHaveLength(1);
    expect(agent.getHistory()).toBe(view);
    expect(agent.getHistory().earlier).toBe(true);
    // not committed, so asked again for the same one
    const again = await agent.readEarlier();
    expect(asked.filter((q) => q.before)).toEqual([
      { limit: "20", before: "6" },
      { limit: "20", before: "6" },
    ]);
    read.commit();
    read.commit();
    expect(agent.getHistory().earlier).toBe(false);
    expect(again.runs).toHaveLength(1);
    // held once: nothing older to ask
    expect(await agent.fetchEarlier()).toEqual([]);
    agent.stop();
  });

  it("refuses a page written by another projection, and the account is as it was", async () => {
    const { agent } = await opened((q) => (q.before ? { ...OLDEST, projection: 2 } : NEWEST));
    await expect(agent.readEarlier()).rejects.toBeInstanceOf(ProjectionChanged);
    expect(agent.getHistory().earlier).toBe(true);
    agent.stop();
  });

  it("a page that fails to load leaves the account as it was, and a later ask reads again", async () => {
    let fail = true;
    const { agent } = await opened((q) => {
      if (!q.before) return NEWEST;
      if (fail) {
        fail = false;
        return problem(503, "Busy");
      }
      return OLDEST;
    });
    await expect(agent.fetchEarlier()).rejects.toThrow();
    expect(agent.getHistory().earlier).toBe(true);
    expect(await agent.fetchEarlier()).toHaveLength(1);
    agent.stop();
  });
});

describe("the gates of an import", () => {
  async function seeded(post?: (call: Call) => Response | Promise<Response>) {
    const rig = open(() => json(NEWEST), {}, post);
    rig.agent.start();
    await waitFor(() => expect(rig.agent.takeSeed()).not.toBeNull());
    rig.agent.seeded();
    await waitFor(() => expect(connects(rig.calls)).toHaveLength(1));
    await waitFor(() => expect(rig.agent.idleForImport()).toBe(true));
    return rig;
  }

  it("is not idle while the seed is awaited, and is once it is in", async () => {
    const { agent, calls } = open(() => json(NEWEST));
    agent.start();
    await waitFor(() => expect(agent.takeSeed()).not.toBeNull());
    expect(agent.idleForImport()).toBe(false);
    agent.seeded();
    await waitFor(() => expect(connects(calls)).toHaveLength(1));
    await waitFor(() => expect(agent.idleForImport()).toBe(true));
    agent.stop();
  });

  /** The first run of the `followup` golden as the stream's next one: its ids come after the page's. */
  const nextRun = (): GoldenFrame[] =>
    golden.slice(0, 13).map((f) => ({
      ...f,
      ...(f.id !== undefined ? { id: f.id + 100 } : {}),
    }));

  it("is not idle while a run is open, and is again when it ends", async () => {
    const { agent, connect } = await seeded();
    const run = nextRun();
    await act(async () => {
      connect.frames(run.slice(0, 5));
    });
    await waitFor(() => expect(agent.getSnapshot().openRun).not.toBeNull());
    expect(agent.idleForImport()).toBe(false);
    await act(async () => {
      connect.frames(run.slice(5));
    });
    await waitFor(() => expect(agent.getSnapshot().openRun).toBeNull());
    await waitFor(() => expect(agent.getSnapshot().lastSeq).toBe(105));
    // the run is the runtime's to take; until it has, an import would lose it
    expect(agent.idleForImport()).toBe(false);
    const taken = await agent.nextExternalRun(new AbortController().signal);
    expect(taken).not.toBeNull();
    agent.applied(taken as NonNullable<typeof taken>);
    await waitFor(() => expect(agent.idleForImport()).toBe(true));
    agent.stop();
  });

  it("is not idle while a message is on its way to the orchestrator, and is again once it has taken it", async () => {
    let accept!: () => void;
    const accepted = new Promise<void>((resolve) => {
      accept = resolve;
    });
    const { agent } = await seeded(async (call) => {
      await accepted;
      const started = new LiveStream();
      started.frames([
        {
          event: {
            type: "RUN_STARTED",
            threadId: THREAD_ID,
            runId: (call.body as { runId: string }).runId,
            protocolVersion: "1.0",
          },
        },
      ]);
      return sse(started.body);
    });
    let sent!: Promise<void>;
    act(() => {
      sent = agent.sendWhileWorking("and then?", "steer");
    });
    expect(agent.idleForImport()).toBe(false);
    accept();
    await act(async () => {
      await sent;
    });
    await waitFor(() => expect(agent.idleForImport()).toBe(true));
    agent.stop();
  });

  it("is not idle while an action on a surface is staged for the next run, and is when it is let go", async () => {
    const { agent } = await seeded();
    agent.stageA2uiAction({ name: "approve", surfaceId: "s1" });
    expect(agent.idleForImport()).toBe(false);
    agent.clearStagedAction();
    expect(agent.idleForImport()).toBe(true);
    agent.stop();
  });

  it("holds the runs back while paused, and hands them over when released (twice is once)", async () => {
    const { agent, connect } = await seeded();
    const release = agent.pauseRuns();
    await act(async () => {
      connect.frames(nextRun());
    });
    await waitFor(() => expect(agent.getSnapshot().lastSeq).toBe(105));
    const taken = vi.fn();
    const abort = new AbortController();
    void agent.nextExternalRun(abort.signal).then(taken);
    await act(async () => {
      await new Promise((r) => setTimeout(r, 60));
    });
    expect(taken).not.toHaveBeenCalled();
    release();
    release();
    await waitFor(() => expect(taken).toHaveBeenCalledTimes(1));
    abort.abort();
    agent.stop();
  });
});
