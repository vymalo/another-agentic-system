// @vitest-environment jsdom
import { act, cleanup, configure, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { buildTurnSteps, type StepMessage, type StepNode } from "../step-tree";
import { framesThrough, LiveStream, loadGolden, sse } from "./testing";
import { mountRuntime, type Summary, summarize } from "./testing-runtime";

configure({ asyncUtilTimeout: 10_000 });
afterEach(cleanup);

/**
 * The AG-UI goldens of docs/api/examples/agui, through the runtime the app uses
 * (`@assistant-ui/react-ag-ui` 0.0.62 with web/patches applied, on `@ag-ui/client` 1.0.0): what a
 * viewer sees for each scripted behaviour has to become the transcript the UI renders.
 */

const ACTOR = "actor";
const USER = (text: string) => ({ role: "user", parts: [`text:${text}`] });
const DONE = "complete";

/** The transcript each viewer stream must produce. */
const EXPECTED: Record<string, Summary> = {
  echo: [
    USER("echo hi"),
    {
      role: "assistant",
      status: DONE,
      parts: [ACTOR, "status:working", "artifact", "status:completed"],
    },
  ],
  release: [
    USER("echo ship it"),
    {
      role: "assistant",
      status: DONE,
      parts: [ACTOR, "status:working", "artifact", "status:completed"],
    },
  ],
  talk: [
    USER("talk to me"),
    {
      role: "assistant",
      status: DONE,
      parts: [
        ACTOR,
        "status:working",
        "status:working",
        "text:Plan: add a test",
        "artifact",
        "status:completed",
      ],
    },
  ],
  ask: [
    USER("ask about branches"),
    // its interrupt was answered by the next run, so the runtime closed it
    {
      role: "assistant",
      status: DONE,
      parts: [ACTOR, "status:working", "text:Which branch?", "status:input_required"],
    },
    USER("main"),
    {
      role: "assistant",
      status: DONE,
      parts: [ACTOR, "status:working", "artifact", "status:completed"],
    },
  ],
  // a thread is a conversation (ADR 0020): the message after `Done` is the next job, an assistant
  // message of its own that starts with the `job` marker
  followup: [
    USER("echo hi"),
    {
      role: "assistant",
      status: DONE,
      parts: [ACTOR, "status:working", "artifact", "status:completed"],
    },
    USER("echo now add tests"),
    {
      role: "assistant",
      status: DONE,
      parts: ["job", ACTOR, "status:working", "artifact", "status:completed"],
    },
  ],
  "followup-after-cancel": [
    USER("slow work"),
    {
      role: "assistant",
      status: "incomplete:cancelled",
      parts: [ACTOR, "status:working", "status:canceled"],
    },
    USER("echo never mind, do this"),
    {
      role: "assistant",
      status: DONE,
      parts: ["job", ACTOR, "status:working", "artifact", "status:completed"],
    },
  ],
  // a rename inside the run is a snapshot of it; the rename after the cancel is a run of its own
  // that holds only a snapshot, which the transcript never shows
  title: [
    USER("slow work"),
    {
      role: "assistant",
      status: "incomplete:cancelled",
      parts: [ACTOR, "status:working", "status:canceled"],
    },
  ],
  // a surface (one part, its two snapshots replaced in place), the question, then the owner's action
  a2ui: [
    USER("ui pick one"),
    {
      role: "assistant",
      status: DONE,
      parts: [ACTOR, "status:working", "a2ui-surface", "text:Pick one", "status:input_required"],
    },
    {
      role: "assistant",
      status: DONE,
      parts: ["action", ACTOR, "status:working", "artifact", "status:completed"],
    },
  ],
  // the verification gate (ADR 0018): ONE run for every attempt (one assistant message), the check
  // card of each attempt, the divider that sends the agent back, and the next attempt's parts
  "verify-green": [
    USER("verify-red-once fix the login"),
    {
      role: "assistant",
      status: DONE,
      parts: [
        ACTOR,
        "status:working",
        "artifact",
        "artifact",
        "status:completed",
        "check:failed",
        "rework",
        ACTOR,
        "status:working",
        "artifact",
        "artifact",
        "status:completed",
        "check:passed",
      ],
    },
  ],
  // CI (ADR 0017): ONE run for both attempts; each report is a `ci` card (its own part, next to the
  // check it decided: the pending check card was replaced in place by the answer)
  ci: [
    USER("verify-ci fix the login"),
    {
      role: "assistant",
      status: DONE,
      parts: [
        ACTOR,
        "status:working",
        "artifact",
        "status:completed",
        "check:failed",
        "ci",
        "rework",
        ACTOR,
        "status:working",
        "artifact",
        "status:completed",
        "check:passed",
        "ci",
      ],
    },
  ],
  // out of attempts: the run ends in RUN_ERROR checks_failed, after the last failed check
  "verify-red": [
    USER("verify-red fix the login"),
    {
      role: "assistant",
      status: "incomplete:error",
      parts: [
        ACTOR,
        "status:working",
        "artifact",
        "artifact",
        "status:completed",
        "check:failed",
        "rework",
        ACTOR,
        "status:working",
        "artifact",
        "artifact",
        "status:completed",
        "check:failed",
        "rework",
        ACTOR,
        "status:working",
        "artifact",
        "artifact",
        "status:completed",
        "check:failed",
        "error",
      ],
    },
  ],
  // the outcome of a run stopped on purpose: neither success nor failure (patch: cancelled outcome)
  cancel: [
    USER("slow work"),
    {
      role: "assistant",
      status: "incomplete:cancelled",
      parts: [ACTOR, "status:working", "status:canceled"],
    },
  ],
  fail: [
    USER("fail please"),
    {
      role: "assistant",
      status: "incomplete:error",
      parts: [ACTOR, "status:working", "status:failed"],
    },
  ],
  // nested steps (ADR 0025): a sub-agent step opens its own subagent in the stream, which the
  // runtime turns into one more actor marker; each step is one part, said again in place
  steps: [
    USER("steps run the tests"),
    {
      role: "assistant",
      status: DONE,
      parts: [ACTOR, "status:working", ACTOR, "step", "step", "text:Done.", "status:completed"],
    },
  ],
  // the agent asked while a command waited: the question ends the run, the answer's run says the
  // command and the sub-agent again (the preamble does not start the sub-agent a second time)
  "steps-ask": [
    USER("steps-ask clean the build"),
    {
      role: "assistant",
      status: DONE,
      parts: [
        ACTOR,
        "status:working",
        ACTOR,
        "step",
        "step",
        "text:Allow rm -rf build?",
        "status:input_required",
      ],
    },
    USER("yes"),
    {
      role: "assistant",
      status: DONE,
      parts: [ACTOR, "status:working", "step", "step", "text:Done.", "status:completed"],
    },
  ],
};

/** The scenarios the orchestrator's e2e tests also record over real HTTP (connect-<name>). */
const CONNECT = ["echo", "ask", "cancel", "verify-green", "verify-red", "ci", "steps", "steps-ask"];

async function play(name: string) {
  const stream = new LiveStream();
  const mounted = mountRuntime(() => sse(stream.body));
  mounted.agent.start();
  const frames = loadGolden(name);
  await act(async () => {
    stream.frames(frames);
  });
  const last = frames.at(-1)?.id;
  await waitFor(() => expect(mounted.agent.getSnapshot().lastSeq).toBe(last));
  await waitFor(() => expect(mounted.runtime().thread.getState().isRunning).toBe(false));
  await waitFor(() => expect(mounted.messages().length).toBeGreaterThan(1));
  // The runs of a replay are applied one after the other, each after the transcript has settled
  // (`quiesce`): wait until the transcript stops changing instead of for a fixed time.
  let seen = -1;
  for (let i = 0; i < 100; i++) {
    const count = mounted.messages().length;
    const running = mounted.runtime().thread.getState().isRunning;
    if (!running && count === seen) break;
    seen = count;
    await act(async () => {
      await new Promise((r) => setTimeout(r, 80));
    });
  }
  return mounted;
}

describe("the goldens through the runtime", () => {
  for (const [name, expected] of Object.entries(EXPECTED)) {
    it(`${name}: the viewer stream becomes the transcript`, async () => {
      const { messages, agent } = await play(name);
      expect(summarize(messages())).toEqual(expected);
      agent.stop();
    });

    if (!CONNECT.includes(name)) continue;
    it(`${name}: the connect stream of the real orchestrator becomes the same transcript`, async () => {
      const { messages, agent } = await play(`connect-${name}`);
      expect(summarize(messages())).toEqual(expected);
      agent.stop();
    });
  }

  it("connect-title: a finished thread that was renamed reads as it did, the rename adds no message and moves the title", async () => {
    const { messages, agent } = await play("connect-title");
    expect(summarize(messages())).toEqual(EXPECTED.echo);
    expect(agent.getSnapshot()).toMatchObject({
      state: "done",
      title: "Fix the build",
      openRun: null,
    });
    agent.stop();
  });

  it("ask, before the answer: the run ended in an interrupt the runtime holds and the UI can answer", async () => {
    const stream = new LiveStream();
    const mounted = mountRuntime(() => sse(stream.body));
    mounted.agent.start();
    await act(async () => {
      // the first run: up to the frame that closes it (resume point 4)
      stream.frames(framesThrough(loadGolden("ask"), 4));
    });
    await waitFor(() => expect(mounted.agent.getSnapshot().lastSeq).toBe(4));
    await waitFor(() => expect(mounted.messages()).toHaveLength(2));
    const pending = mounted.runtime().unstable_getPendingInterrupts();
    expect(pending).toMatchObject([
      {
        id: "int-3",
        reason: "input_required",
        message: "Which branch?",
        responseSchema: { required: ["text"] },
      },
    ]);
    expect(mounted.messages()[1]?.status).toMatchObject({
      type: "requires-action",
      reason: "interrupt",
    });
    mounted.agent.stop();
  });

  it("activities that precede any agent text are kept (nothing is dropped on reload)", async () => {
    const { messages, agent } = await play("echo");
    const parts = summarize(messages())[1]?.parts ?? [];
    expect(parts.slice(0, 2)).toEqual([ACTOR, "status:working"]);
    agent.stop();
  });

  it("the actor and revision travel to the renderers", async () => {
    const { messages, agent } = await play("release");
    const assistant = messages()[1];
    const data = assistant?.content.filter((p) => p.type === "data") ?? [];
    const status = data.find((p) => p.name === "agui-activity/vymalo.status");
    expect(status).toMatchObject({
      data: { status: "working", actor: { type: "agent", name: "coder", revision: "coder-r51" } },
    });
    const marker = data.find((p) => p.name === "vymalo.actor");
    expect(marker).toMatchObject({ data: { name: "coder", revision: "coder-r51" } });
    agent.stop();
  });

  it("a reply the agent gives twice is said once", async () => {
    const { messages, agent } = await play("talk");
    const texts = messages().flatMap((m) => m.content.filter((p) => p.type === "text"));
    expect(texts.map((p) => p.text)).toEqual(["talk to me", "Plan: add a test"]);
    agent.stop();
  });

  it("verify-green: the agent holds the job (attempt 2 of 3, the commit) and no failure", async () => {
    const { agent } = await play("verify-green");
    expect(agent.getSnapshot()).toMatchObject({
      state: "done",
      job: {
        attempt: 2,
        maxAttempts: 3,
        gate: ["agent_checks"],
        sha: "0000000000000000000000000000000000000002",
      },
      failure: null,
    });
    agent.stop();
  });

  it("verify-red: the run error 'checks_failed' is kept, on attempt 3 of 3", async () => {
    const { agent } = await play("verify-red");
    expect(agent.getSnapshot()).toMatchObject({
      state: "failed",
      job: { attempt: 3, maxAttempts: 3 },
      failure: { code: "checks_failed" },
    });
    expect(agent.getSnapshot().failure?.message).toContain("after 3 attempts");
    agent.stop();
  });

  it("ci: the agent holds the job under a CI gate, and the reports are data parts the renderer reads", async () => {
    const { agent, messages } = await play("ci");
    expect(agent.getSnapshot()).toMatchObject({
      state: "done",
      job: { attempt: 2, maxAttempts: 3, gate: ["ci"] },
      failure: null,
    });
    const reports = messages().flatMap((m) =>
      m.content.filter((p) => p.type === "data" && p.name === "agui-activity/vymalo.ci"),
    );
    expect(reports.map((p) => (p as { data: unknown }).data)).toMatchObject([
      { name: "ci/build", conclusion: "failure", passed: false, actor: { type: "system" } },
      { name: "ci/build", conclusion: "success", passed: true, actor: { type: "system" } },
    ]);
    agent.stop();
  });

  /** One line per node, indented by depth: `kind:label[state]`. */
  const outline = (nodes: readonly StepNode[], depth = 0): string[] =>
    nodes.flatMap((n) => [
      `${"  ".repeat(depth)}${n.kind}:${n.label}[${n.state}]`,
      ...outline(n.children, depth + 1),
    ]);

  for (const name of ["steps", "connect-steps"]) {
    it(`${name}: the sub-agent holds its command, and the turn says what failed`, async () => {
      const { messages, agent } = await play(name);
      const turns = buildTurnSteps(messages() as unknown as StepMessage[], {
        state: "done",
        waiting: false,
        agentId: "plain",
      });
      expect(turns).toHaveLength(1);
      const [turn] = turns;
      expect(outline(turn?.roots ?? [])).toEqual([
        "agent:plain[completed]",
        "  status:Started working[completed]",
        "  subagent:OpenCode[completed]",
        "    command:npm test[failed]",
      ]);
      expect(turn).toMatchObject({
        number: 1,
        state: "completed",
        summary: { total: 3, running: 0, failed: 1 },
      });
      agent.stop();
    });
  }

  for (const name of ["steps-ask", "connect-steps-ask"]) {
    it(`${name}: the turn that asked stays paused with its command waiting, the next one ends it`, async () => {
      const { messages, agent } = await play(name);
      const turns = buildTurnSteps(messages() as unknown as StepMessage[], {
        state: "done",
        waiting: false,
        agentId: "plain",
      });
      expect(turns.map((t) => [t.number, t.state])).toEqual([
        [1, "waiting"],
        [2, "completed"],
      ]);
      expect(outline(turns[0]?.roots ?? [])).toEqual([
        "agent:plain[waiting]",
        "  status:Started working[completed]",
        "  subagent:OpenCode[waiting]",
        "    command:rm -rf build[waiting]",
      ]);
      // the answer's run says both again, attributed to the agent: the parent is not in this turn
      expect(outline(turns[1]?.roots ?? [])).toEqual([
        "agent:plain[completed]",
        "  status:Started working[completed]",
        "  command:rm -rf build[completed]",
        "  subagent:OpenCode[completed]",
      ]);
      expect(turns.map((t) => t.summary.failed)).toEqual([0, 0]);
      agent.stop();
    });
  }

  it("a turn that did not change is the same object when a later one grows (nothing is rebuilt)", async () => {
    const stream = new LiveStream();
    const mounted = mountRuntime(() => sse(stream.body));
    mounted.agent.start();
    const all = loadGolden("steps-ask");
    // up to the answer's run being open: the first turn is closed (its interrupt was answered)
    const first = framesThrough(all, 8);
    await act(async () => {
      stream.frames(first);
    });
    await waitFor(() => expect(mounted.messages()).toHaveLength(4));
    await waitFor(() => expect(mounted.messages()[1]?.status?.type).toBe("complete"));
    const asMessages = () => mounted.messages() as unknown as StepMessage[];
    const before = buildTurnSteps(asMessages(), {
      state: "working",
      waiting: false,
      agentId: "plain",
    });
    expect(before).toHaveLength(2);
    await act(async () => {
      stream.frames(all.slice(first.length));
    });
    await waitFor(() => expect(mounted.agent.getSnapshot().lastSeq).toBe(all.at(-1)?.id));
    await waitFor(() => expect(mounted.messages()[3]?.status?.type).toBe("complete"));
    const after = buildTurnSteps(asMessages(), { state: "done", waiting: false, agentId: "plain" });
    expect(after).toHaveLength(2);
    // the runtime keeps a message that did not change as the same object, so its turn is too
    expect(after[0]).toBe(before[0]);
    expect(after[1]).not.toBe(before[1]);
    expect(after[1]?.state).toBe("completed");
    mounted.agent.stop();
  });

  it("followup: the agent holds job 2 of the thread, done, and nothing of the first job", async () => {
    const { agent } = await play("followup");
    expect(agent.getSnapshot()).toMatchObject({ state: "done", failure: null, job: null });
    agent.stop();
  });

  it("a thread without a gate has no job, and an agent failure is not checks_failed", async () => {
    const echo = await play("echo");
    expect(echo.agent.getSnapshot().job).toBeNull();
    echo.agent.stop();
    const fail = await play("fail");
    expect(fail.agent.getSnapshot()).toMatchObject({
      job: null,
      failure: { code: "agent_failed" },
    });
    fail.agent.stop();
  });
});
