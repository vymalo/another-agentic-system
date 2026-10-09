import { describe, expect, it } from "vitest";
import { loadGolden } from "./agui/testing";
import {
  callsOf,
  foldUsage,
  hasUsage,
  levelOf,
  NO_USAGE,
  summarize,
  type ThreadUsage,
  USAGE_EVENT,
  USAGE_TOTAL_EVENT,
} from "./usage";

type Frame = { id?: number; event: Record<string, unknown> };

const foldFrames = (frames: readonly Frame[], from: ThreadUsage = NO_USAGE): ThreadUsage =>
  frames.reduce((s, f) => foldUsage(s, f.event.name, f.event.value), from);

const golden = () => loadGolden("usage") as Frame[];

const call = (
  task: string,
  id: string,
  input: number,
  output: number,
  extra: Record<string, unknown> = {},
) => ({
  task,
  call: id,
  agent: "coder",
  model: "glm-5.3",
  provider: "openai",
  inputTokens: input,
  outputTokens: output,
  totalTokens: input + output,
  by: { kind: "agent", name: "coder" },
  ...extra,
});

describe("token usage of a thread (ADR 0056)", () => {
  it("folds the golden stream: the totals per model, the agent and its sub-agent apart, the latest call", () => {
    const usage = foldFrames(golden());
    expect(callsOf(usage).map((c) => c.call)).toEqual(["c1", "c2", "c3"]);
    const s = summarize(usage);
    // the task's totals, as the agent kept them
    expect(s.models).toEqual([
      {
        provider: "openai",
        model: "glm-5.3",
        counts: {
          inputTokens: 3600,
          outputTokens: 200,
          totalTokens: 3800,
          reasoningTokens: 20,
          cachedInputTokens: 1000,
        },
      },
      {
        provider: "openai",
        model: "glm-5.3-mini",
        counts: { inputTokens: 600, outputTokens: 40, totalTokens: 640 },
      },
    ]);
    expect(s.groups.map((g) => [g.kind, g.name, g.calls, g.counts.inputTokens])).toEqual([
      ["agent", "plain", 2, 3600],
      ["subagent", "Researcher", 1, 600],
    ]);
    expect(s.latest?.call).toBe("c3");
    expect(s.fill).toEqual({ ratio: 2400 / 131072, level: "normal" });
  });

  it("a replay and the live stream give the same usage: a call or totals said again change nothing", () => {
    const frames = golden();
    const once = foldFrames(frames);
    // a reconnect that says part of the stream again, and a second replay of all of it
    const cut = frames.findIndex((f) => f.event.name === USAGE_TOTAL_EVENT);
    const live = foldFrames(frames, foldFrames(frames.slice(0, cut)));
    expect(live).toEqual(once);
    expect(foldFrames(frames, once)).toBe(once);
    expect(summarize(live)).toEqual(summarize(once));
  });

  it("a state keeps its calls when an older state is folded on: the newest is extended in place, an older one copied", () => {
    const one = foldUsage(NO_USAGE, USAGE_EVENT, call("t", "c1", 1, 1));
    const two = foldUsage(one, USAGE_EVENT, call("t", "c2", 2, 1));
    // from `one` again, another call: `two` keeps c2, the new state has c3 and not c2
    const other = foldUsage(one, USAGE_EVENT, call("t", "c3", 3, 1));
    expect(callsOf(one).map((c) => c.call)).toEqual(["c1"]);
    expect(callsOf(two).map((c) => c.call)).toEqual(["c1", "c2"]);
    expect(callsOf(other).map((c) => c.call)).toEqual(["c1", "c3"]);
    // c2 is not a repeat for a state that never had it
    expect(
      callsOf(foldUsage(other, USAGE_EVENT, call("t", "c2", 2, 1))).map((c) => c.call),
    ).toEqual(["c1", "c3", "c2"]);
    expect(NO_USAGE.size).toBe(0);
    expect(NO_USAGE.log).toHaveLength(0);
  });

  it("without totals a task counts its calls; a call after the totals adds to them", () => {
    let s = foldUsage(NO_USAGE, USAGE_EVENT, call("t1", "c1", 100, 10));
    s = foldUsage(s, USAGE_EVENT, call("t1", "c2", 200, 20));
    expect(summarize(s).models[0]?.counts).toEqual({
      inputTokens: 300,
      outputTokens: 30,
      totalTokens: 330,
    });
    s = foldUsage(s, USAGE_TOTAL_EVENT, {
      task: "t1",
      agent: "coder",
      totals: [
        {
          provider: "openai",
          model: "glm-5.3",
          inputTokens: 250,
          outputTokens: 25,
          totalTokens: 275,
        },
      ],
    });
    // the totals are the record: lower than the calls' sum is what the agent kept
    expect(summarize(s).models[0]?.counts.inputTokens).toBe(250);
    s = foldUsage(s, USAGE_EVENT, call("t1", "c3", 50, 5));
    expect(summarize(s).models[0]?.counts.inputTokens).toBe(300);
    // the per-agent lines are the calls as reported
    expect(summarize(s).groups[0]?.counts.inputTokens).toBe(350);
  });

  it("fills with the latest call of the thread's agent, never a sub-agent's or an asked agent's", () => {
    let s = foldUsage(
      NO_USAGE,
      USAGE_EVENT,
      call("t1", "c1", 104_858, 10, { contextWindow: 131_072 }),
    );
    expect(summarize(s).fill?.level).toBe("warn");
    s = foldUsage(
      s,
      USAGE_EVENT,
      call("t1", "c2", 10, 1, {
        contextWindow: 1000,
        by: { kind: "subagent", name: "Researcher" },
      }),
    );
    s = foldUsage(
      s,
      USAGE_EVENT,
      call("t2", "c1", 999, 1, {
        contextWindow: 1000,
        agent: "researcher",
        path: ["ask-1"],
        by: { kind: "ask", name: "researcher" },
      }),
    );
    expect(summarize(s).latest?.call).toBe("c1");
    expect(summarize(s).fill?.level).toBe("warn");
    expect(summarize(s).groups.map((g) => g.kind)).toEqual(["agent", "subagent", "ask"]);
    // a call with no window leaves the ring without a fill
    s = foldUsage(s, USAGE_EVENT, call("t1", "c4", 10, 1));
    expect(summarize(s).fill).toBeUndefined();
    expect(hasUsage(s)).toBe(true);
  });

  it("says amber from 80 % and red from 95 %", () => {
    expect(levelOf(0.79)).toBe("normal");
    expect(levelOf(0.8)).toBe("warn");
    expect(levelOf(0.949)).toBe("warn");
    expect(levelOf(0.95)).toBe("danger");
    const full = foldUsage(
      NO_USAGE,
      USAGE_EVENT,
      call("t", "c", 140_000, 1, { contextWindow: 131_072 }),
    );
    expect(summarize(full).fill).toEqual({ ratio: 1, level: "danger" });
  });

  it("ignores what does not say what the contract says, and any other event", () => {
    for (const bad of [
      null,
      "c1",
      call("t", "c", 1.5, 1),
      call("t", "c", -1, 1),
      { ...call("t", "c", 1, 1), model: "" },
      { ...call("t", "c", 1, 1), call: undefined },
    ]) {
      expect(foldUsage(NO_USAGE, USAGE_EVENT, bad)).toBe(NO_USAGE);
    }
    expect(foldUsage(NO_USAGE, USAGE_TOTAL_EVENT, { task: "t", totals: "x" })).toBe(NO_USAGE);
    expect(foldUsage(NO_USAGE, "marker", call("t", "c", 1, 1))).toBe(NO_USAGE);
    expect(hasUsage(NO_USAGE)).toBe(false);
  });
});
