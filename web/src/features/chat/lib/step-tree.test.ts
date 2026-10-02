import { describe, expect, it } from "vitest";
import { agentTurns } from "@/features/panel/lib/sources";
import { ACTIVITY, ACTOR_PART, activityPartName } from "./agui/vymalo";
import {
  buildTurnSteps,
  countUnder,
  firstFailed,
  formatDuration,
  hasIo,
  nodeDuration,
  pathTo,
  type StepMessage,
  type StepNode,
  summaryLine,
  type TurnSteps,
  visibleChildren,
} from "./step-tree";

const data = (type: string, value: Record<string, unknown>) => ({
  type: "data",
  name: activityPartName(type),
  data: value,
});
const actor = (name = "coder", revision?: string) => ({
  type: "data",
  name: ACTOR_PART,
  data: { type: "agent", name, ...(revision ? { revision } : {}) },
});
const AT = (s: number) => `2027-01-15T08:00:${String(s).padStart(2, "0")}Z`;

const step = (
  id: string,
  state: string,
  extra: Record<string, unknown> = {},
  at = 1,
  path: string[] = [],
) =>
  data(ACTIVITY.step, {
    id,
    path,
    kind: "tool",
    label: id,
    state,
    startedAt: AT(at),
    at: AT(at),
    ...extra,
  });
const status = (s: string, detail?: string, at = 1) =>
  data(ACTIVITY.status, { status: s, ...(detail ? { detail } : {}), at: AT(at) });
const text = (t: string) => ({ type: "text", text: t });

let n = 0;
const assistant = (
  content: StepMessage["content"],
  state: StepMessage["status"] = { type: "complete" },
  id = `m${++n}`,
): StepMessage => ({ id, role: "assistant", content, status: state });
const user = (t: string): StepMessage => ({ id: `u${++n}`, role: "user", content: [text(t)] });

const VIEW = { state: "done", waiting: false, agentId: "coder" };
const build = (messages: StepMessage[], view = VIEW) => buildTurnSteps(messages, view);
const root = (turn: TurnSteps): StepNode => turn.roots[0] as StepNode;
const labels = (nodes: readonly StepNode[]) => nodes.map((x) => x.label);

describe("one tree per agent turn", () => {
  it("numbers the turns as the panel's Sources do and keys them by the message", () => {
    const messages = [
      user("go"),
      assistant([actor(), status("working"), step("a", "completed")], undefined, "t1"),
      user("and"),
      assistant([text("only words")], undefined, "t2"),
      assistant([actor(), status("working")], undefined, "t3"),
    ];
    const turns = build(messages);
    expect(turns.map((t) => [t.turnId, t.number])).toEqual([
      ["t1", 1],
      ["t2", 2],
      ["t3", 3],
    ]);
    // the numbers the Sources tab prints on its "Turn n" buttons
    expect(agentTurns(messages).map((t) => [t.id, t.number])).toEqual(
      turns.map((t) => [t.turnId, t.number]),
    );
    expect(turns[0]?.agent).toEqual({ name: "coder" });
  });

  it("names the agent and its revision from the actor marker, else from the thread's agent", () => {
    const [a, b] = build([
      assistant([actor("coder", "coder-r47"), status("working")]),
      assistant([status("working")]),
    ]);
    expect(a?.agent).toEqual({ name: "coder", revision: "coder-r47" });
    expect(b?.agent).toEqual({ name: "coder" });
  });

  it("is a root, the turn's agent, that holds the steps in the order they first appeared", () => {
    const [turn] = build([
      assistant([
        actor(),
        status("working", undefined, 1),
        step("one", "completed", {}, 2),
        status("working", "reading the code", 3),
      ]),
    ]);
    expect(turn?.roots).toHaveLength(1);
    expect(root(turn as TurnSteps)).toMatchObject({ kind: "agent", label: "coder" });
    expect(labels(root(turn as TurnSteps).children)).toEqual([
      "Started working",
      "one",
      "reading the code",
    ]);
  });

  it("leaves what is not a step to the chat: words, the answers, the statuses that come with words, a failure", () => {
    const [turn] = build([
      assistant([
        actor(),
        text("here are my thoughts"),
        status("completed", "done"),
        status("input_required", "which one?"),
        status("failed", "it broke"),
        data(ACTIVITY.job, { job: 2, at: AT(1) }),
        data(ACTIVITY.error, { message: "no", retryable: false }),
        data(ACTIVITY.action, {
          surfaceId: "s",
          name: "choices",
          context: { answers: [{ id: "q", values: ["a"] }] },
        }),
        step("kept", "completed"),
      ]),
    ]);
    expect(labels(root(turn as TurnSteps).children)).toEqual(["kept"]);
  });
});

describe("the tree of steps/v1", () => {
  it("nests a step under the last id of its path, however deep", () => {
    const [turn] = build([
      assistant([
        step("T/tool", "running", { kind: "subagent", label: "OpenCode" }, 1),
        step("T/cmd", "running", { kind: "command", label: "npm test" }, 2, ["T/tool"]),
        step("T/leaf", "completed", { label: "read" }, 3, ["T/tool", "T/cmd"]),
      ]),
    ]);
    const [opencode] = root(turn as TurnSteps).children;
    expect(opencode).toMatchObject({ kind: "subagent", label: "OpenCode" });
    expect(opencode?.children[0]).toMatchObject({ kind: "command", label: "npm test" });
    expect(opencode?.children[0]?.children[0]).toMatchObject({ id: "T/leaf" });
  });

  it("puts a step whose parent is not in the turn under the agent (a step that began in an earlier run)", () => {
    const [turn] = build([
      assistant([step("T/child", "completed", { label: "late" }, 5, ["T/gone"])]),
    ]);
    expect(labels(root(turn as TurnSteps).children)).toEqual(["late"]);
  });

  it("never nests a step under itself", () => {
    const [turn] = build([assistant([step("loop", "running", {}, 1, ["loop"])])]);
    expect(labels(root(turn as TurnSteps).children)).toEqual(["loop"]);
  });

  it("updates a step in place when it says itself again (an update, its end, a retry)", () => {
    const [turn] = build([
      assistant([
        step("T/cmd", "running", { kind: "command", label: "npm test" }, 1),
        step("T/other", "completed", {}, 2),
        step("T/cmd", "failed", { kind: "command", label: "npm test", detail: "1 failed" }, 4),
        step("T/cmd", "running", { kind: "command", label: "npm test" }, 6),
      ]),
    ]);
    const children = root(turn as TurnSteps).children;
    expect(children.map((c) => c.id)).toEqual(["T/cmd", "T/other"]);
    // the retry started again: running, no detail, the time of the first start kept
    expect(children[0]).toMatchObject({ state: "canceled", startedAt: AT(1), at: AT(6) });
    expect(children[0]?.detail).toBeUndefined();
  });

  it("reads an unknown kind as a tool and drops an icon outside the vocabulary", () => {
    const [turn] = build([
      assistant([step("a", "completed", { kind: "mystery", icon: "mcp-server:x" })]),
    ]);
    expect(root(turn as TurnSteps).children[0]).toMatchObject({ kind: "tool" });
    expect(root(turn as TurnSteps).children[0]?.icon).toBeUndefined();
  });

  it("draws nothing for a report that does not validate", () => {
    const messages = [
      assistant([
        data(ACTIVITY.step, { id: "a", label: "no state", kind: "tool", path: [] }),
        data(ACTIVITY.step, { id: "b", state: "completed", kind: "tool", path: [] }),
        data(ACTIVITY.step, { label: "no id", state: "completed", kind: "tool", path: [] }),
        data(ACTIVITY.step, { id: "c", label: "x", state: "exploded", kind: "tool", path: [] }),
      ]),
    ];
    expect(build(messages)).toEqual([]);
  });
});

describe("today's activities are leaves", () => {
  it("draws a status, an artifact and an action as leaves of the agent, a command as a command step", () => {
    const [turn] = build([
      assistant([
        status("working", "$ cargo test -p auth", 1),
        status("working", "reading the code", 2),
        status("submitted", undefined, 3),
        status("auth_required", "GitHub", 4),
        data(ACTIVITY.artifact, {
          kind: "branch",
          name: "agent/fix",
          branch: "agent/fix",
          repository: "github.com/acme/demo",
          at: AT(5),
        }),
        data(ACTIVITY.artifact, { kind: "checks", name: "checks", passed: false, at: AT(6) }),
        data(ACTIVITY.artifact, {
          kind: "pull_request",
          name: "PR",
          url: "https://github.com/acme/demo/pull/12",
          at: AT(7),
        }),
        data(ACTIVITY.action, { surfaceId: "s", name: "Go", at: AT(8) }),
      ]),
    ]);
    const children = root(turn as TurnSteps).children;
    expect(children.map((c) => [c.kind, c.label, c.state])).toEqual([
      ["command", "cargo test -p auth", "completed"],
      ["status", "reading the code", "completed"],
      ["status", "Queued", "completed"],
      ["status", "Needs you to sign in: GitHub", "waiting"],
      ["artifact", "Pushed agent/fix", "completed"],
      ["artifact", "Checks failed", "failed"],
      ["artifact", "Opened pull request #12", "completed"],
      ["action", "Chose Go", "completed"],
    ]);
    expect(children[0]?.icon).toBe("execute");
    expect(children[0]?.part).toEqual({ messageId: (turn as TurnSteps).turnId, index: 0 });
  });

  it("makes the last step of a running turn the one the agent is on, and nothing else", () => {
    const running = { type: "running" };
    const [turn] = build(
      [assistant([status("working", undefined, 1), status("working", "second", 2)], running)],
      { ...VIEW, state: "working" },
    );
    expect(root(turn as TurnSteps).children.map((c) => c.state)).toEqual(["completed", "running"]);
    expect(turn?.summary).toMatchObject({ running: 1, current: "second" });
  });

  it("puts the checks, the CI reports and the reworks after the agent, in order", () => {
    const [turn] = build([
      assistant([
        status("working"),
        data(ACTIVITY.check, {
          source: "agent_checks",
          attempt: 1,
          status: "failed",
          stale: false,
          findings: ["x"],
          at: AT(2),
        }),
        data(ACTIVITY.ci, {
          name: "ci/build",
          conclusion: "failure",
          passed: false,
          sha: "a".repeat(40),
          shortSha: "aaaaaaa",
          provider: "github",
          repository: "acme/demo",
          at: AT(3),
        }),
        data(ACTIVITY.rework, { attempt: 2, maxAttempts: 3, findings: [], at: AT(4) }),
        status("working", undefined, 5),
        data(ACTIVITY.check, {
          source: "verifier",
          attempt: 2,
          status: "passed",
          stale: false,
          findings: [],
          at: AT(6),
        }),
      ]),
    ]);
    expect(turn?.roots.map((r) => [r.kind, r.state])).toEqual([
      ["agent", "completed"],
      ["check", "failed"],
      ["ci", "failed"],
      ["rework", "completed"],
      ["check", "completed"],
    ]);
    expect(labels(turn?.roots.slice(1) as StepNode[])).toEqual([
      "The agent's checks failed",
      "CI ci/build: Failure",
      "Checks failed — trying again (2/3)",
      "The verifier approved the work",
    ]);
    // the gate's nodes are steps too: two failed (the check and the CI report)
    expect(turn?.summary).toMatchObject({ total: 6, failed: 2 });
  });

  it("spins a pending check while the run is open, and says nobody answered after it", () => {
    const pending = data(ACTIVITY.check, {
      source: "ci",
      attempt: 1,
      status: "pending",
      stale: false,
      findings: [],
    });
    const [open] = build([assistant([status("working"), pending], { type: "running" })], {
      ...VIEW,
      state: "verifying",
    });
    expect(open?.state).toBe("verifying");
    expect(open?.roots[1]?.state).toBe("running");
    const [ended] = build([assistant([status("working"), pending])]);
    expect(ended?.roots[1]?.state).toBe("canceled");
  });

  it("counts a stale answer as nothing", () => {
    const [turn] = build([
      assistant([
        data(ACTIVITY.check, {
          source: "ci",
          attempt: 1,
          status: "failed",
          stale: true,
          findings: [],
        }),
      ]),
    ]);
    expect(turn?.roots[1]).toMatchObject({ state: "canceled" });
    expect(turn?.summary.failed).toBe(0);
  });
});

describe("the state of a turn", () => {
  const one = (state: StepMessage["status"], view = VIEW) =>
    build([assistant([status("working"), step("s", "running")], state)], view)[0] as TurnSteps;

  it("is running while the message runs, verifying when the thread is being verified", () => {
    expect(one({ type: "running" }, { ...VIEW, state: "working" }).state).toBe("running");
    expect(one({ type: "running" }, { ...VIEW, state: "verifying" }).state).toBe("verifying");
  });

  it("is verifying only for the last message", () => {
    const [first, second] = build(
      [
        assistant([status("working")], { type: "complete" }),
        assistant([status("working")], { type: "running" }),
      ],
      { ...VIEW, state: "verifying" },
    );
    expect(first?.state).toBe("completed");
    expect(second?.state).toBe("verifying");
  });

  it("is waiting when the agent asked, failed or canceled by the run's outcome, else completed", () => {
    expect(one({ type: "requires-action", reason: "interrupt" }).state).toBe("waiting");
    expect(one({ type: "incomplete", reason: "error" }).state).toBe("failed");
    expect(one({ type: "incomplete", reason: "cancelled" }).state).toBe("canceled");
    expect(one({ type: "complete" }).state).toBe("completed");
    expect(one(undefined).state).toBe("completed");
  });

  it("stays paused in the history for a turn that ended on a question, though the answer closed it", () => {
    const [asked] = build([
      assistant([
        status("working"),
        step("s", "running"),
        text("Which?"),
        status("input_required", "Which?"),
      ]),
    ]);
    expect(asked?.state).toBe("waiting");
  });

  it("holds no running step unless it runs: waiting turns pause theirs, ended turns cancel them", () => {
    function* walk(ns: readonly StepNode[]): Generator<string> {
      for (const x of ns) {
        yield x.state;
        yield* walk(x.children);
      }
    }
    const states = (turn: TurnSteps) => [...walk(turn.roots)];
    expect(states(one({ type: "requires-action", reason: "interrupt" }))).toEqual([
      "waiting",
      "completed",
      "waiting",
    ]);
    expect(states(one({ type: "incomplete", reason: "cancelled" }))).toEqual([
      "canceled",
      "completed",
      "canceled",
    ]);
    expect(states(one({ type: "running" }, { ...VIEW, state: "working" }))).toEqual([
      "running",
      "completed",
      "running",
    ]);
  });

  it("times a finished turn from the first to the last event of its activities, a running one not at all", () => {
    const parts = [
      status("working", undefined, 2),
      step("s", "completed", {}, 9),
      status("completed", "ok", 12),
    ];
    const [done] = build([assistant(parts)]);
    expect(done).toMatchObject({ startedAt: AT(2), endedAt: AT(12) });
    expect(done?.summary.durationMs).toBe(10_000);
    const [live] = build([assistant(parts, { type: "running" })], { ...VIEW, state: "working" });
    expect(live?.startedAt).toBe(AT(2));
    expect(live?.endedAt).toBeUndefined();
    expect(live?.summary.durationMs).toBeUndefined();
  });
});

describe("the summary", () => {
  const tree = (state: StepMessage["status"], view = VIEW) =>
    build(
      [
        assistant(
          [
            actor(),
            step("T/oc", "running", { kind: "subagent", label: "OpenCode" }, 1),
            step("T/a", "completed", { label: "read file" }, 2, ["T/oc"]),
            step("T/b", "failed", { kind: "command", label: "npm test" }, 3, ["T/oc"]),
            step("T/c", "running", { kind: "command", label: "npm run build" }, 4, ["T/oc"]),
          ],
          state,
        ),
      ],
      view,
    )[0] as TurnSteps;

  it("counts every step of the tree, the turn's own root not among them", () => {
    expect(tree({ type: "running" }, { ...VIEW, state: "working" }).summary).toMatchObject({
      total: 4,
      failed: 1,
      running: 2,
    });
  });

  it("names the deepest running step: 'Running npm run build'", () => {
    expect(tree({ type: "running" }, { ...VIEW, state: "working" }).summary.current).toBe(
      "Running npm run build",
    );
  });

  it("falls back to the last step when nothing runs, and cuts a long command to its first line", () => {
    const done = tree({ type: "complete" });
    expect(done.summary.running).toBe(0);
    expect(done.summary.current).toBe("npm run build");
    const [long] = build([
      assistant(
        [step("c", "running", { kind: "command", label: `${"x".repeat(200)}\nsecond line` })],
        {
          type: "running",
        },
      ),
    ]);
    expect(long?.summary.current?.length).toBeLessThanOrEqual(90);
    expect(long?.summary.current).not.toContain("second line");
  });

  it("counts under a node: its descendants, the failed ones and the running ones", () => {
    const turn = tree({ type: "running" }, { ...VIEW, state: "working" });
    const opencode = root(turn).children[0] as StepNode;
    expect(countUnder(opencode)).toEqual({ total: 3, failed: 1, running: 1 });
  });
});

describe("summaryLine", () => {
  const turn = (state: TurnSteps["state"], extra: Partial<TurnSteps["summary"]> = {}) => ({
    state,
    summary: { total: 14, running: 0, failed: 0, ...extra },
  });

  it("says what a running turn is on and how many steps it has", () => {
    expect(summaryLine(turn("running", { current: "Running npm test" }))).toEqual({
      icon: "spinner",
      text: "Running npm test · 14 steps",
      failed: 0,
    });
    expect(summaryLine(turn("running", { total: 0 }))).toEqual({
      icon: "spinner",
      text: "Working",
      failed: 0,
    });
    expect(summaryLine(turn("running", { total: 1, current: "Started working" }))?.text).toBe(
      "Started working · 1 step",
    );
  });

  it("says paused, verifying, failed and stopped; a turn that did nothing says nothing", () => {
    expect(summaryLine(turn("waiting", { total: 9 }))).toMatchObject({
      icon: "pause",
      text: "Paused · 9 steps",
    });
    expect(summaryLine(turn("verifying"))).toMatchObject({ icon: "verifying", text: "Verifying" });
    expect(summaryLine(turn("failed"))).toMatchObject({ icon: "cross", text: "Failed · 14 steps" });
    expect(summaryLine(turn("canceled"))).toMatchObject({
      icon: "stopped",
      text: "Stopped · 14 steps",
    });
    for (const state of ["waiting", "failed", "canceled", "completed"] as const) {
      expect(summaryLine(turn(state, { total: 0 }))).toBeNull();
    }
  });

  it("says a finished turn's steps and how long they took, and a failed step even in a good turn", () => {
    expect(summaryLine(turn("completed", { durationMs: 130_000 }))).toEqual({
      icon: "check",
      text: "14 steps · 2m 10s",
      failed: 0,
    });
    expect(summaryLine(turn("completed", { failed: 1 }))).toEqual({
      icon: "check",
      text: "14 steps",
      failed: 1,
    });
    expect(summaryLine(turn("running", { total: 5, failed: 2, current: "x" }))?.failed).toBe(2);
  });
});

describe("opening a level", () => {
  const parent = (children: { state: StepNode["state"] }[]) => ({
    children: children.map((c, i) => ({
      id: `c${i}`,
      kind: "tool",
      label: `c${i}`,
      children: [],
      ...c,
    })) as StepNode[],
  });

  it("lists the latest children and says how many are hidden", () => {
    const node = parent(Array.from({ length: 14 }, () => ({ state: "completed" as const })));
    const { nodes, hidden } = visibleChildren(node, 3);
    expect(labels(nodes)).toEqual(["c11", "c12", "c13"]);
    expect(hidden).toBe(11);
    expect(visibleChildren(node, 13).hidden).toBe(1);
    expect(visibleChildren(node, 99)).toMatchObject({ hidden: 0 });
    expect(visibleChildren(node, 99).nodes).toHaveLength(14);
  });

  it("keeps every failed child in view, in its place", () => {
    const node = parent(
      Array.from({ length: 10 }, (_, i) => ({
        state: i === 2 || i === 4 ? ("failed" as const) : ("completed" as const),
      })),
    );
    const { nodes, hidden } = visibleChildren(node, 3);
    expect(labels(nodes)).toEqual(["c2", "c4", "c7", "c8", "c9"]);
    expect(hidden).toBe(5);
  });

  it("lists nothing for none shown", () => {
    const node = parent([{ state: "completed" }]);
    expect(visibleChildren(node, 0)).toEqual({ nodes: [], hidden: 1 });
  });
});

describe("durations", () => {
  it("writes how long something took, and nothing under a second", () => {
    expect(formatDuration(999)).toBeUndefined();
    expect(formatDuration(1000)).toBe("1s");
    expect(formatDuration(42_400)).toBe("42s");
    expect(formatDuration(60_000)).toBe("1m");
    expect(formatDuration(130_000)).toBe("2m 10s");
    expect(formatDuration(3_900_000)).toBe("1h 5m");
    expect(formatDuration(Number.NaN)).toBeUndefined();
  });

  it("times a finished node from its own times, a running one not at all", () => {
    expect(nodeDuration({ state: "completed", startedAt: AT(1), at: AT(31) })).toBe("30s");
    expect(nodeDuration({ state: "running", startedAt: AT(1), at: AT(31) })).toBeUndefined();
    expect(nodeDuration({ state: "completed" })).toBeUndefined();
  });
});

describe("rebuilding", () => {
  it("builds a turn once for a message that did not change, and again when it did", () => {
    const first = assistant([status("working"), step("s", "completed")]);
    const second = assistant([status("working")], { type: "running" });
    const a = build([first, second], { ...VIEW, state: "working" });
    const b = build([first, second], { ...VIEW, state: "working" });
    expect(b[0]).toBe(a[0]);
    expect(b[1]).toBe(a[1]);
    const changed = { ...second, content: [...second.content, step("t", "running")] };
    const c = build([first, changed], { ...VIEW, state: "working" });
    expect(c[0]).toBe(a[0]);
    expect(c[1]).not.toBe(a[1]);
  });

  it("rebuilds the turn the thread's state changes under it", () => {
    const message = assistant([status("working")], { type: "running" });
    const a = build([message], { ...VIEW, state: "working" })[0];
    const b = build([message], { ...VIEW, state: "verifying" })[0];
    expect(a?.state).toBe("running");
    expect(b?.state).toBe("verifying");
  });
});

describe("input and output (ADR 0030)", () => {
  const IN = { query: "Stephane Segning", limit: 3 };
  const OUT = { text: "1. a result", truncated: true, bytes: 9000 };

  it("keeps the input of the start and the output of the end on one node", () => {
    const [turn] = build([
      assistant([
        step("T/s", "running", { input: IN }, 1),
        step("T/s", "completed", { input: IN, output: OUT }, 2),
      ]),
    ]);
    const [node] = root(turn as TurnSteps).children;
    expect(node).toMatchObject({ id: "T/s", state: "completed", input: IN, output: OUT });
    expect(hasIo(node as StepNode)).toBe(true);
  });

  it("keeps the input when a later report does not repeat it, and takes a retry's new output", () => {
    const [turn] = build([
      assistant([
        step("T/s", "running", { input: IN }, 1),
        step("T/s", "running", {}, 2),
        step("T/s", "failed", { output: { text: "boom", error: true } }, 3),
        step("T/s", "completed", { output: { text: "ok" } }, 4),
      ]),
    ]);
    const [node] = root(turn as TurnSteps).children;
    expect(node?.input).toEqual(IN);
    expect(node?.output).toEqual({ text: "ok" });
  });

  it("notes a step the record's budget had no room for, and a step with nothing has nothing to open", () => {
    const [turn] = build([
      assistant([
        step("T/a", "completed", { ioDropped: true }, 1),
        step("T/b", "completed", {}, 2),
      ]),
    ]);
    const [a, b] = root(turn as TurnSteps).children as StepNode[];
    expect(a).toMatchObject({ ioDropped: true });
    expect(hasIo(a as StepNode)).toBe(true);
    expect(hasIo(b as StepNode)).toBe(false);
  });

  it("names a tool step by its tool in the line the chat keeps", () => {
    const [turn] = build(
      [
        assistant([step("T/s", "running", { label: "search__web_search" }, 1)], {
          type: "running",
        }),
      ],
      { state: "working", waiting: false, agentId: "coder" },
    );
    expect(turn?.summary.current).toBe("Web search");
  });
});

describe("a failed run_checks is one failure", () => {
  const redChecks = (at: number) =>
    data(ACTIVITY.artifact, { kind: "checks", name: "checks", passed: false, at: AT(at) });
  const greenChecks = (at: number) =>
    data(ACTIVITY.artifact, { kind: "checks", name: "checks", passed: true, at: AT(at) });
  const runChecks = (id: string, state: string, at: number) =>
    step(id, state, { label: "run_checks" }, at);

  it("counts its step and its checks artifact once, in the chip of the turn and of the node", () => {
    const [turn] = build([
      assistant([runChecks("T/rc", "running", 1), redChecks(2), runChecks("T/rc", "failed", 3)]),
    ]);
    const t = turn as TurnSteps;
    // two rows (the step and the artifact it made), one failure
    expect(t.summary).toMatchObject({ total: 2, failed: 1 });
    expect(countUnder(root(t))).toMatchObject({ total: 2, failed: 1 });
    expect(summaryLine(t)).toMatchObject({ failed: 1 });
    const [stepNode, artifact] = root(t).children as StepNode[];
    expect(stepNode?.echo).toBeUndefined();
    expect(artifact).toMatchObject({ kind: "artifact", state: "failed", echo: true });
  });

  it("counts the owner's coder thread once per failed run: two reds, two failures (not four)", () => {
    const [turn] = build([
      assistant([
        runChecks("T/1", "running", 1),
        redChecks(2),
        runChecks("T/1", "failed", 3),
        step("T/fix", "failed", { label: "apply_patch" }, 4),
        step("T/fix2", "completed", { label: "apply_patch" }, 5),
        runChecks("T/2", "running", 6),
        redChecks(7),
        runChecks("T/2", "failed", 8),
        runChecks("T/3", "running", 9),
        greenChecks(10),
        runChecks("T/3", "completed", 11),
      ]),
    ]);
    expect((turn as TurnSteps).summary.failed).toBe(3);
  });

  it("counts a red artifact whose run_checks did not fail, or that has no step, as the failure it is", () => {
    const failed = (parts: StepMessage["content"]) => build([assistant(parts)])[0]?.summary.failed;
    expect(failed([runChecks("T/rc", "completed", 1), redChecks(2)])).toBe(1);
    expect(failed([redChecks(3)])).toBe(1);
    expect(failed([step("T/o", "failed", { label: "write_file" }, 1), redChecks(2)])).toBe(2);
  });

  it("does not let one failed run_checks excuse two red artifacts", () => {
    const [turn] = build([assistant([runChecks("T/rc", "failed", 1), redChecks(2), redChecks(3)])]);
    expect((turn as TurnSteps).summary.failed).toBe(2);
  });
});

describe("finding a step", () => {
  const nested = () =>
    build([
      assistant([
        step("T/oc", "completed", { kind: "subagent", label: "OpenCode" }, 1),
        step("T/ok", "completed", { label: "ok" }, 2, ["T/oc"]),
        step("T/bad", "failed", { kind: "command", label: "npm test" }, 3, ["T/oc"]),
        step("T/bad2", "failed", { label: "later" }, 4),
      ]),
    ])[0] as TurnSteps;

  it("walks from the turn's root to the step", () => {
    const turn = nested();
    expect(pathTo(turn, "T/bad").map((n) => n.id)).toEqual([root(turn).id, "T/oc", "T/bad"]);
    expect(pathTo(turn, "nowhere")).toEqual([]);
  });

  it("names the first failed step: where the chat's failed chip goes", () => {
    expect(firstFailed(nested())?.id).toBe("T/bad");
    const none = build([assistant([step("T/a", "completed", {}, 1)])])[0] as TurnSteps;
    expect(firstFailed(none)).toBeUndefined();
  });

  it("does not offer an echo, and falls back to a failed check of the gate", () => {
    const [echoed] = build([
      assistant([
        step("T/rc", "failed", { label: "run_checks" }, 1),
        data(ACTIVITY.artifact, { kind: "checks", name: "checks", passed: false, at: AT(2) }),
      ]),
    ]);
    expect(firstFailed(echoed as TurnSteps)?.id).toBe("T/rc");
  });
});
