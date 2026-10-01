import type { BaseEvent, RunAgentInput } from "@ag-ui/client";
import { EventType } from "@ag-ui/client";
import { describe, expect, it, vi } from "vitest";
import { OWN_CATALOG, UI_CATALOG_PROP } from "@/features/chat/lib/a2ui/catalog";
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
import { type ExternalRun, SendError, ThreadAgent } from "./thread-agent";

const BASE = "http://orch.test";

function agentWith(
  handler: (call: Call) => Response | Promise<Response>,
  extra: Partial<ConstructorParameters<typeof ThreadAgent>[0]> = {},
) {
  const { fetch, calls } = fakeFetch(handler);
  const agent = new ThreadAgent({
    threadId: THREAD_ID,
    fetch,
    baseUrl: BASE,
    target: () => ({ agentId: "plain", release: null }),
    backoff: () => 1,
    ...extra,
  });
  return { agent, calls };
}

const until = async (test: () => boolean, what = "condition") => {
  for (let i = 0; i < 400; i++) {
    if (test()) return;
    await new Promise((r) => setTimeout(r, 5));
  }
  throw new Error(`timed out waiting for ${what}`);
};

/** Everything an external run hands the runtime, once it is complete. */
async function collect(run: ExternalRun): Promise<BaseEvent[]> {
  const out: BaseEvent[] = [];
  await new Promise<void>((resolve, reject) => {
    run.frames.subscribe({ next: (e) => out.push(e), complete: resolve, error: reject });
  });
  return out;
}

const kinds = (events: BaseEvent[]) => events.map((e) => e.type);

/** The viewer's stream of the echo script cut into the pieces a connection may deliver. */
const echo = () => loadGolden("connect-echo");

describe("ThreadAgent: the connect stream", () => {
  it("makes an external run of a run nobody here started, and splits off the user message", async () => {
    const stream = new LiveStream();
    const { agent, calls } = agentWith(() => sse(stream.body));
    agent.start();
    stream.frames(echo());

    const run = (await agent.nextExternalRun()) as ExternalRun;
    await run.leadIn;
    expect(run.runId).toBe("run-1");
    expect(run.userMessages).toEqual([
      { id: "msg-1", text: "echo hi", actor: { type: "user", name: "alice@example.com" } },
    ]);
    const events = await collect(run);
    expect(kinds(events)).toEqual([
      "RUN_STARTED",
      "STATE_SNAPSHOT",
      // the user's three frames are not here: the runtime gets the message, not a reply that starts with it
      "SUBAGENT_STARTED",
      "CUSTOM",
      "ACTIVITY_SNAPSHOT",
      "STATE_SNAPSHOT",
      "ACTIVITY_SNAPSHOT",
      "ACTIVITY_SNAPSHOT",
      "SUBAGENT_FINISHED",
      "STATE_SNAPSHOT",
      "RUN_FINISHED",
    ]);
    // the actor, which the runtime would drop with the event's metadata, travels in the content
    expect(events[3]).toMatchObject({
      name: "vymalo.actor",
      value: { type: "agent", name: "plain" },
    });
    expect(events[4]).toMatchObject({
      activityType: "vymalo.status",
      content: { status: "working", actor: { type: "agent", name: "plain" } },
    });
    expect(agent.getSnapshot()).toMatchObject({ lastSeq: 5, state: "done", openRun: null });
    // one GET, from the start
    expect(calls).toEqual([{ method: "GET", path: `/agui/threads/${THREAD_ID}/connect` }]);
    agent.stop();
  });

  it("hands over whole groups only: a cut connection delivers none of the group it cut, the reconnect resumes at the last id", async () => {
    const full = echo();
    // log event 3 (the artifact) is one frame with an id; event 4 is two frames, the id on the last
    const at3 = full.findIndex((f) => f.id === 3);
    const preamble: GoldenFrame[] = [
      full[0] as GoldenFrame, // RUN_STARTED
      full[5] as GoldenFrame, // SUBAGENT_STARTED
      full[7] as GoldenFrame, // STATE_SNAPSHOT (working)
    ];
    const first = new LiveStream();
    const second = new LiveStream();
    const streams = [first, second];
    const { agent, calls } = agentWith(() => sse((streams.shift() as LiveStream).body));
    agent.start();

    // connection 1: everything up to id 3, then the first frame of the next group, then the cable
    first.frames(full.slice(0, at3 + 1));
    first.frames(full.slice(at3 + 1, at3 + 2));
    expect(full[at3 + 1]?.id).toBeUndefined();
    const run = (await agent.nextExternalRun()) as ExternalRun;
    await until(() => agent.getSnapshot().lastSeq === 3, "the first three events");
    await new Promise((r) => setTimeout(r, 30)); // the half group has been read, and held back
    const seen: BaseEvent[] = [];
    run.frames.subscribe({ next: (e) => seen.push(e) });
    expect(seen.at(-1)).toMatchObject({ activityType: "vymalo.artifact" });
    first.cut();

    // connection 2 answers the cursor: the run opened again (its preamble), then the rest
    await until(() => calls.length === 2, "the reconnect");
    expect(calls[1]).toEqual({
      method: "GET",
      path: `/agui/threads/${THREAD_ID}/connect`,
      lastEventId: "3",
    });
    second.frames([...preamble, ...full.slice(at3 + 1)]);
    await until(() => agent.getSnapshot().lastSeq === 5, "the end of the run");

    // the same run as without the cut: no repeat, no gap, the preamble not shown twice
    const whole = new LiveStream();
    const baseline = agentWith(() => sse(whole.body)).agent;
    baseline.start();
    whole.frames(full);
    const same = (await baseline.nextExternalRun()) as ExternalRun;
    expect(seen).toEqual(await collect(same));
    expect(kinds(seen).filter((t) => t === "RUN_STARTED")).toHaveLength(1);
    expect(kinds(seen).filter((t) => t === "SUBAGENT_STARTED")).toHaveLength(1);
    agent.stop();
    baseline.stop();
  });

  it("drops what it has delivered already, whatever the server sends (dedupe by seq)", async () => {
    const full = echo();
    const at3 = full.findIndex((f) => f.id === 3);
    const first = new LiveStream();
    const second = new LiveStream();
    const streams = [first, second];
    const { agent } = agentWith(() => sse((streams.shift() as LiveStream).body));
    agent.start();
    first.frames(full.slice(0, at3 + 1));
    const run = (await agent.nextExternalRun()) as ExternalRun;
    await until(() => agent.getSnapshot().lastSeq === 3, "event 3");
    const seen: BaseEvent[] = [];
    run.frames.subscribe({ next: (e) => seen.push(e) });
    first.cut();
    // a server that ignores Last-Event-ID and replays the whole stream
    await until(() => streams.length === 0, "the reconnect");
    second.frames(full);
    await until(() => agent.getSnapshot().lastSeq === 5, "the end of the run");

    const whole = new LiveStream();
    const baseline = agentWith(() => sse(whole.body)).agent;
    baseline.start();
    whole.frames(full);
    expect(seen).toEqual(await collect((await baseline.nextExternalRun()) as ExternalRun));
    agent.stop();
    baseline.stop();
  });

  it("answers 404 with notFound and does not retry; other failures retry and are reported", async () => {
    const missing = agentWith(() => problem(404, "Thread not found"));
    missing.agent.start();
    await until(() => missing.agent.getSnapshot().notFound, "notFound");
    await new Promise((r) => setTimeout(r, 30));
    expect(missing.calls).toHaveLength(1);

    let n = 0;
    const flaky = agentWith(() =>
      ++n < 3 ? problem(503, "Unavailable", "the store is down") : sse(new LiveStream().body),
    );
    flaky.agent.start();
    await until(
      () => flaky.calls.length === 3 && flaky.agent.getSnapshot().connection === "open",
      "the third try",
    );
    expect(flaky.agent.getSnapshot().error).toBeNull();
    flaky.agent.stop();
  });

  it("keeps a run that continues across the preamble open, and a new run after it separate", async () => {
    const ask = loadGolden("connect-ask");
    const stream = new LiveStream();
    const { agent } = agentWith(() => sse(stream.body));
    agent.start();
    stream.frames(ask);
    const one = (await agent.nextExternalRun()) as ExternalRun;
    const two = (await agent.nextExternalRun()) as ExternalRun;
    expect([one.runId, two.runId]).toEqual(["run-1", "run-2"]);
    const events = await collect(one);
    expect(events.at(-1)).toMatchObject({
      type: "RUN_FINISHED",
      outcome: { type: "interrupt", interrupts: [{ id: "int-3", reason: "input_required" }] },
    });
    expect(two.userMessages).toMatchObject([{ text: "main" }]);
    agent.stop();
  });

  it("drops a run that holds nothing but snapshots (a rename of a finished thread): the title moves, the runtime never hears of it", async () => {
    const stream = new LiveStream();
    const { agent } = agentWith(() => sse(stream.body));
    agent.start();
    stream.frames(loadGolden("connect-title"));
    // the run of the echo, then the rename's own run `run-6`: RUN_STARTED, STATE_SNAPSHOT, RUN_FINISHED
    const one = (await agent.nextExternalRun()) as ExternalRun;
    expect(one.runId).toBe("run-1");
    await until(() => agent.getSnapshot().lastSeq === 6, "the rename's run");
    expect(agent.getSnapshot()).toMatchObject({
      title: "Fix the build",
      state: "done",
      openRun: null,
      failure: null,
    });
    expect(await Promise.race([agent.nextExternalRun(AbortSignal.timeout(30)), null])).toBeNull();
    // the first run is whole: the rename did not become a second message of it
    expect(kinds(await collect(one)).at(-1)).toBe("RUN_FINISHED");
    agent.stop();
  });

  it("a rename inside a run is a snapshot of that run, and the run is delivered as before", async () => {
    const stream = new LiveStream();
    const { agent } = agentWith(() => sse(stream.body));
    agent.start();
    stream.frames(loadGolden("title"));
    const run = (await agent.nextExternalRun()) as ExternalRun;
    expect(run.runId).toBe("run-1");
    const titles = (await collect(run)).flatMap((e) => {
      const snapshot = (e as BaseEvent & { snapshot?: { thread?: { title?: string } } }).snapshot;
      return e.type === EventType.STATE_SNAPSHOT && snapshot?.thread?.title
        ? [snapshot.thread.title]
        : [];
    });
    expect(titles).toEqual(["slow work", "slow work", "Fix the login", "Fix the login"]);
    // the second run, after the cancel, holds only the rename of a cancelled thread
    await until(() => agent.getSnapshot().lastSeq === 6, "the second rename");
    expect(agent.getSnapshot()).toMatchObject({ title: "Fix the login page", state: "cancelled" });
    expect(await Promise.race([agent.nextExternalRun(AbortSignal.timeout(30)), null])).toBeNull();
    agent.stop();
  });

  it("a run with a user message but nothing else is still a run for the transcript", async () => {
    const stream = new LiveStream();
    const { agent } = agentWith(() => sse(stream.body));
    agent.start();
    // cancelled before the agent started: the user's message, a snapshot, RUN_FINISHED
    stream.frames([
      { event: { type: "RUN_STARTED", threadId: THREAD_ID, runId: "run-1" } },
      { event: { type: "STATE_SNAPSHOT", snapshot: { thread: { state: "queued", title: "t" } } } },
      { event: { type: "TEXT_MESSAGE_START", messageId: "m-1", role: "user" } },
      { event: { type: "TEXT_MESSAGE_CONTENT", messageId: "m-1", delta: "hello" } },
      { id: 1, event: { type: "TEXT_MESSAGE_END", messageId: "m-1" } },
      {
        event: { type: "STATE_SNAPSHOT", snapshot: { thread: { state: "cancelled", title: "t" } } },
      },
      {
        id: 2,
        event: {
          type: "RUN_FINISHED",
          threadId: THREAD_ID,
          runId: "run-1",
          outcome: { type: "cancelled" },
        },
      },
    ]);
    const run = (await agent.nextExternalRun()) as ExternalRun;
    expect(run.userMessages).toMatchObject([{ text: "hello" }]);
    expect(kinds(await collect(run)).at(-1)).toBe("RUN_FINISHED");
    agent.stop();
  });
});

describe("ThreadAgent.run", () => {
  const input = (over: Partial<RunAgentInput> = {}): RunAgentInput => ({
    threadId: THREAD_ID,
    runId: "run-9",
    state: {},
    tools: [],
    context: [],
    forwardedProps: {},
    messages: [
      { id: "old-user", role: "user", content: "before" },
      { id: "old-assistant", role: "assistant", content: "answer" },
      { id: "new-user", role: "user", content: "next" },
    ],
    ...over,
  });

  const started = (runId: string) => ({
    type: "RUN_STARTED",
    threadId: THREAD_ID,
    runId,
    protocolVersion: "1.0",
  });

  it("is accepted at RUN_STARTED, releases the response and takes the run from the connect stream", async () => {
    const connect = new LiveStream();
    const post = new LiveStream();
    const accepted = vi.fn();
    const { agent, calls } = agentWith(
      (call) => (call.method === "POST" ? sse(post.body) : sse(connect.body)),
      { onAccepted: accepted },
    );
    agent.start();
    const seen: BaseEvent[] = [];
    let finished = false;
    agent.run(input()).subscribe({ next: (e) => seen.push(e), complete: () => (finished = true) });

    // nothing is accepted until the server says RUN_STARTED
    await until(() => calls.some((c) => c.method === "POST"), "the POST");
    expect(accepted).not.toHaveBeenCalled();
    post.write(`data: ${JSON.stringify(started("run-9"))}\n\n`);
    await until(() => seen.length === 1, "acceptance");
    expect(accepted).toHaveBeenCalledWith({ threadId: THREAD_ID, runId: "run-9" });
    expect(seen[0]).toMatchObject({ type: "RUN_STARTED", runId: "run-9" });
    await until(() => post.cancelled, "the response to be released");

    // the run's events are the connect stream's; its user message is the requester's, not sent back
    const golden = loadGolden("connect-echo").map((f) =>
      "runId" in f.event ? { ...f, event: { ...f.event, runId: "run-9" } } : f,
    );
    connect.frames(golden);
    await until(() => finished, "the run to end");
    expect(kinds(seen)).toEqual([
      "RUN_STARTED",
      "STATE_SNAPSHOT",
      "SUBAGENT_STARTED",
      "CUSTOM",
      "ACTIVITY_SNAPSHOT",
      "STATE_SNAPSHOT",
      "ACTIVITY_SNAPSHOT",
      "ACTIVITY_SNAPSHOT",
      "SUBAGENT_FINISHED",
      "STATE_SNAPSHOT",
      "RUN_FINISHED",
    ]);
    // and nothing became an external run
    expect(await Promise.race([agent.nextExternalRun(AbortSignal.timeout(20)), null])).toBeNull();
    agent.stop();
  });

  it("keeps a send that stop() finds in flight: the connect stream may finish its run before the POST answers", async () => {
    // The page pauses the connect stream (stop()) once the thread is finished and caught up. The
    // run of an answer can be finished on the connect stream while the POST's own RUN_STARTED is
    // still on its way; the pause must not take the send, and the reply it holds, with it.
    const connect = new LiveStream();
    const post = new LiveStream();
    const { agent } = agentWith((call) =>
      call.method === "POST" ? sse(post.body) : sse(connect.body),
    );
    agent.start();
    const ask = loadGolden("connect-ask");
    const second = ask.findIndex(
      (f) => f.event.type === "RUN_STARTED" && f.event.runId === "run-2",
    );
    connect.frames(ask.slice(0, second));
    await until(() => agent.getSnapshot().lastSeq === 4, "the first run");

    const seen: BaseEvent[] = [];
    let finished = false;
    agent
      .run(
        input({
          runId: "run-2",
          resume: [{ interruptId: "int-3", status: "resolved", payload: { text: "main" } }],
        }),
      )
      .subscribe({ next: (e) => seen.push(e), complete: () => (finished = true) });
    connect.frames(ask.slice(second));
    await until(() => agent.getSnapshot().lastSeq === 9, "the second run on the connect stream");
    expect(agent.getSnapshot()).toMatchObject({ state: "done", openRun: null });
    agent.stop(); // what the page does now: nothing is left to follow

    post.write(`data: ${JSON.stringify(started("run-2"))}\n\n`);
    await until(() => finished, "the run to end");
    expect(seen.at(0)).toMatchObject({ type: "RUN_STARTED", runId: "run-2" });
    expect(seen).toContainEqual(
      expect.objectContaining({
        type: "ACTIVITY_SNAPSHOT",
        activityType: "vymalo.artifact",
        content: expect.objectContaining({ text: "answered: main" }),
      }),
    );
    expect(seen.at(-1)?.type).toBe("RUN_FINISHED");
  });

  it("sends the one new user message, or the resume alone, never the history", async () => {
    const sent: unknown[] = [];
    const { agent } = agentWith((call) => {
      sent.push(call.body);
      const runId = (call.body as { runId: string }).runId;
      const post = new LiveStream();
      post.write(`data: ${JSON.stringify(started(runId))}\n\n`);
      return sse(post.body);
    });
    const go = (i: RunAgentInput) =>
      new Promise<void>((resolve) => {
        const s = agent.run(i).subscribe({
          next: () => {
            s.unsubscribe();
            resolve();
          },
        });
      });
    await go(input());
    await go(
      input({
        runId: "run-10",
        resume: [{ interruptId: "int-3", status: "resolved", payload: { text: "main" } }],
      }),
    );
    expect(sent[0]).toMatchObject({
      threadId: THREAD_ID,
      runId: "run-9",
      messages: [{ id: "new-user", role: "user", content: "next" }],
    });
    expect(sent[0]).not.toHaveProperty("resume");
    expect(sent[1]).toMatchObject({
      runId: "run-10",
      messages: [],
      resume: [{ interruptId: "int-3", status: "resolved", payload: { text: "main" } }],
    });
  });

  it("puts the selected release in forwardedProps under the extension URI, only when there is one", async () => {
    const bodies: { forwardedProps: unknown }[] = [];
    let release: string | null = "staging";
    const { agent } = agentWith(
      (call) => {
        bodies.push(call.body as { forwardedProps: unknown });
        return problem(400, "Invalid request");
      },
      { target: () => ({ agentId: "coder", release }) },
    );
    const go = () => new Promise<void>((r) => agent.run(input()).subscribe({ error: () => r() }));
    await go();
    release = null;
    await go();
    // (a thread nobody told about the catalog yet gets it with every run: see "the UI catalog")
    expect(bodies[0]?.forwardedProps).toEqual({
      "https://agents.vymalo.com/a2a/extensions/release-channels/v1": { release: "staging" },
      [UI_CATALOG_PROP]: OWN_CATALOG,
    });
    expect(bodies[1]?.forwardedProps).toEqual({ [UI_CATALOG_PROP]: OWN_CATALOG });
  });

  it("fails with the problem when the server refuses, once, and claims nothing", async () => {
    const { agent } = agentWith(() =>
      problem(409, "Conflict", "the thread is finished (Done); start a new thread"),
    );
    const error = await new Promise<unknown>((resolve) => {
      agent.run(input()).subscribe({ error: resolve });
    });
    expect(error).toBeInstanceOf(SendError);
    expect(error).toMatchObject({
      status: 409,
      message: "the thread is finished (Done); start a new thread",
    });
    expect(agent.takeSendError()).toBe(error);
    expect(agent.takeSendError()).toBeNull();
  });

  it("needs an agent to send to", async () => {
    const { agent, calls } = agentWith(() => problem(500, "x"), {
      target: () => ({ agentId: null, release: null }),
    });
    const error = await new Promise<unknown>((resolve) =>
      agent.run(input()).subscribe({ error: resolve }),
    );
    expect(error).toMatchObject({ message: "Choose an agent first." });
    expect(calls).toHaveLength(0);
  });

  it("abortRun is a truncation: the run goes on and nothing is cancelled; cancel() asks the orchestrator", async () => {
    const connect = new LiveStream();
    const post = new LiveStream();
    const { agent, calls } = agentWith((call) => {
      if (call.path.endsWith("/cancel")) return new Response(null, { status: 202 });
      return call.method === "POST" ? sse(post.body) : sse(connect.body);
    });
    agent.start();
    const seen: BaseEvent[] = [];
    const sub = agent.run(input()).subscribe({ next: (e) => seen.push(e) });
    post.write(`data: ${JSON.stringify(started("run-9"))}\n\n`);
    await until(() => seen.length === 1, "acceptance");
    agent.abortRun();
    sub.unsubscribe();
    expect(calls.filter((c) => c.path.endsWith("/cancel"))).toHaveLength(0);
    await agent.cancel();
    expect(calls.filter((c) => c.path.endsWith("/cancel"))).toEqual([
      { method: "POST", path: `/api/threads/${THREAD_ID}/cancel` },
    ]);
    agent.stop();
  });

  it("a cancel the orchestrator refuses is an error with its reason", async () => {
    const { agent } = agentWith(() => problem(404, "Not found", "no such thread"));
    await expect(agent.cancel()).rejects.toMatchObject({ status: 404, message: "no such thread" });
  });
});

describe("ThreadAgent.adopt", () => {
  it("makes the next run() serve an external run instead of sending", async () => {
    const stream = new LiveStream();
    const { agent, calls } = agentWith(() => sse(stream.body));
    agent.start();
    stream.frames(echo());
    const run = (await agent.nextExternalRun()) as ExternalRun;
    agent.adopt(run);
    const seen: BaseEvent[] = [];
    await new Promise<void>((resolve) =>
      agent
        .run({
          threadId: THREAD_ID,
          runId: "whatever",
          messages: [],
          tools: [],
          context: [],
          state: {},
          forwardedProps: {},
        })
        .subscribe({ next: (e) => seen.push(e), complete: resolve }),
    );
    expect(seen.at(0)?.type).toBe(EventType.RUN_STARTED);
    expect(seen.at(-1)?.type).toBe(EventType.RUN_FINISHED);
    expect(calls.map((c) => c.method)).toEqual(["GET"]);
    agent.stop();
  });
});

describe("ThreadAgent: the UI catalog (ADR 0023)", () => {
  const ref = (version: number, digest = OWN_CATALOG.digest) => ({
    catalogId: OWN_CATALOG.catalogId,
    version,
    digest,
  });
  const other = `sha256:${"b".repeat(64)}`;
  const runInput = (over: Partial<RunAgentInput> = {}): RunAgentInput => ({
    threadId: THREAD_ID,
    runId: "run-9",
    state: {},
    tools: [],
    context: [],
    forwardedProps: {},
    messages: [{ id: "u1", role: "user", content: "next" }],
    ...over,
  });

  /** An agent that has read a snapshot saying `uiCatalog` (nothing: the thread has none). */
  async function afterSnapshot(uiCatalog: unknown, extra: Record<string, unknown> = {}) {
    const stream = new LiveStream();
    const bodies: { forwardedProps: Record<string, unknown> }[] = [];
    const { agent } = agentWith((call) => {
      if (call.method === "POST") {
        bodies.push(call.body as { forwardedProps: Record<string, unknown> });
        return problem(400, "Invalid request");
      }
      return sse(stream.body);
    }, extra);
    agent.start();
    stream.frames([
      { event: { type: "RUN_STARTED", threadId: THREAD_ID, runId: "run-1" } },
      {
        event: {
          type: "STATE_SNAPSHOT",
          snapshot: {
            thread: {
              state: "blocked",
              title: "t",
              target: { agentId: "plain" },
              ...(uiCatalog === undefined ? {} : { uiCatalog }),
            },
          },
        },
        id: 1,
      },
    ]);
    await until(() => agent.getSnapshot().lastSeq === 1, "the snapshot");
    const send = (input: RunAgentInput = runInput()) =>
      new Promise<void>((r) => agent.run(input).subscribe({ error: () => r() }));
    return { agent, bodies, send };
  }

  it("reads thread.uiCatalog of a snapshot, and a snapshot without one clears it", async () => {
    const { agent } = await afterSnapshot(ref(3));
    expect(agent.getSnapshot().uiCatalog).toEqual(ref(3));
    agent.stop();
    const none = await afterSnapshot(undefined);
    expect(none.agent.getSnapshot().uiCatalog).toBeUndefined();
    none.agent.stop();
  });

  it("ignores a catalog it cannot compare: no digest, a bad digest, a version of 0 or 1.5", async () => {
    for (const bad of [
      { catalogId: "x", version: 1 },
      { catalogId: "x", version: 1, digest: "sha256:ABC" },
      { catalogId: "x", version: 0, digest: other },
      { catalogId: "x", version: 1.5, digest: other },
      "text",
    ]) {
      const { agent } = await afterSnapshot(bad);
      expect(agent.getSnapshot().uiCatalog).toBeUndefined();
      agent.stop();
    }
  });

  it("sends the catalog with the run that creates a thread (no snapshot yet), whole", async () => {
    const { agent, calls } = agentWith(() => problem(400, "Invalid request"));
    await new Promise<void>((r) => agent.run(runInput()).subscribe({ error: () => r() }));
    const body = calls.find((c) => c.method === "POST")?.body as {
      forwardedProps: Record<string, unknown>;
    };
    expect(body.forwardedProps[UI_CATALOG_PROP]).toEqual({
      catalogId: "https://agents.vymalo.com/a2ui/catalogs/chat",
      version: OWN_CATALOG.version,
      digest: OWN_CATALOG.digest,
      catalog: OWN_CATALOG.catalog,
    });
  });

  it("does not send it when the thread has this very digest", async () => {
    const { agent, bodies, send } = await afterSnapshot(ref(OWN_CATALOG.version));
    await send();
    expect(bodies[0]?.forwardedProps).toEqual({});
    agent.stop();
  });

  it("does not send it from an older build: the thread's version is higher", async () => {
    const { agent, bodies, send } = await afterSnapshot(ref(OWN_CATALOG.version + 1, other));
    await send();
    expect(bodies[0]?.forwardedProps).toEqual({});
    agent.stop();
  });

  it("sends it when the thread's version is lower", async () => {
    const lower = { ...OWN_CATALOG, version: 5, digest: `sha256:${"5".repeat(64)}` };
    const { agent, bodies, send } = await afterSnapshot(ref(4, other), { catalog: lower });
    await send();
    expect(bodies[0]?.forwardedProps).toEqual({ [UI_CATALOG_PROP]: lower });
    agent.stop();
  });

  it("sends it when the version is the same and the digest is not", async () => {
    const { agent, bodies, send } = await afterSnapshot(ref(OWN_CATALOG.version, other));
    await send();
    expect(bodies[0]?.forwardedProps).toEqual({ [UI_CATALOG_PROP]: OWN_CATALOG });
    agent.stop();
  });

  it("is merged with the action the run carries, and with the release", async () => {
    const { agent, bodies, send } = await afterSnapshot(undefined, {
      target: () => ({ agentId: "coder", release: "staging" }),
    });
    await send();
    await send(
      runInput({
        runId: "run-10",
        messages: [],
        forwardedProps: { a2uiAction: { userAction: { name: "go", surfaceId: "s1" } } },
      }),
    );
    expect(bodies[0]?.forwardedProps).toEqual({
      "https://agents.vymalo.com/a2a/extensions/release-channels/v1": { release: "staging" },
      [UI_CATALOG_PROP]: OWN_CATALOG,
    });
    expect(bodies[1]?.forwardedProps).toEqual({
      a2uiAction: { userAction: { name: "go", surfaceId: "s1" } },
      [UI_CATALOG_PROP]: OWN_CATALOG,
    });
    agent.stop();
  });

  it("an action run of a thread that has the catalog carries the action alone", async () => {
    const { agent, bodies, send } = await afterSnapshot(ref(OWN_CATALOG.version));
    agent.stageA2uiAction({ name: "go", surfaceId: "s1", sourceComponentId: "go", context: {} });
    await send(runInput({ messages: [] }));
    expect(bodies[0]?.forwardedProps).toEqual({
      a2uiAction: {
        userAction: { name: "go", surfaceId: "s1", sourceComponentId: "go", context: {} },
      },
    });
    agent.stop();
  });
});
