// @vitest-environment jsdom
import { act, cleanup, configure, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { LiveStream, loadGolden, sse } from "./testing";
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
    { role: "assistant", status: DONE, parts: [ACTOR, "status:working", "status:input_required"] },
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
  // a surface (one part, its two snapshots replaced in place), the question, then the owner's action
  a2ui: [
    USER("ui pick one"),
    {
      role: "assistant",
      status: DONE,
      parts: [ACTOR, "status:working", "a2ui-surface", "status:input_required"],
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
};

/** The scenarios the orchestrator's e2e tests also record over real HTTP (connect-<name>). */
const CONNECT = ["echo", "ask", "cancel", "verify-green", "verify-red", "ci"];

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
  await new Promise((r) => setTimeout(r, 30));
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

  it("ask, before the answer: the run ended in an interrupt the runtime holds and the UI can answer", async () => {
    const stream = new LiveStream();
    const mounted = mountRuntime(() => sse(stream.body));
    mounted.agent.start();
    await act(async () => {
      stream.frames(loadGolden("ask").slice(0, 12));
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
