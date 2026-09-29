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
const CONNECT = ["echo", "ask", "cancel"];

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
});
