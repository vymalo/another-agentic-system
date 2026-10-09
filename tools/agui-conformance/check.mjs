// Feeds every AG-UI golden (docs/api/examples/agui/*.agui.json) through the reference client,
// @ag-ui/client 1.0.0, and fails on any error or warning.
//
// The goldens are what the orchestrator's projection (orch-agui-projection) writes for a viewer:
// an array of {id?, event}. Here each is framed as the surface will write it (`id:` and `data:`
// lines, LF, a keepalive comment between runs) and read through the real pipeline of the
// reference HttpAgent:
//
//     runHttpRequest -> transformHttpEventStream (parseSSEStream) -> enforceEvents
//       -> transformChunks -> verifyEvents -> defaultApplyEvents
//
// Two passes per golden:
//   1. the whole stream, as the connect endpoint sends it (several runs on one stream), through
//      `connectAgent`; the final messages, state, interrupts and run outcomes must equal
//      expected/<name>.json (UPDATE_EXPECTED=1 rewrites them);
//   2. each run alone, through `runAgent`, as the run endpoint sends one.
//
// `enforceEvents` reports what it strips or drops as console warnings; any warning fails the
// check. A self-test first feeds streams that break the protocol and requires that they fail, so
// a green run means the harness can say no.
import { readFile, readdir, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { HttpAgent, runHttpRequest, transformHttpEventStream } from "@ag-ui/client";

const here = path.dirname(fileURLToPath(import.meta.url));
const goldenDir = path.resolve(here, "../../docs/api/examples/agui");
const expectedDir = path.join(here, "expected");
const update = process.env.UPDATE_EXPECTED === "1";

/** One SSE frame as the server writes it: an optional `id:` line, then `data:`, LF endings. */
function frameText({ id, event }) {
  return `${id === undefined ? "" : `id: ${id}\n`}data: ${JSON.stringify(event)}\n\n`;
}

/** Frames as an SSE body; `keepalives` puts a comment block after every terminal event. */
function sseBody(frames, { keepalives = true } = {}) {
  let out = keepalives ? ": keepalive\n\n" : "";
  for (const frame of frames) {
    out += frameText(frame);
    if (keepalives && ["RUN_FINISHED", "RUN_ERROR"].includes(frame.event.type)) {
      out += ": keepalive\n\n";
    }
  }
  return out;
}

/** The reference HttpAgent reading a canned response; `connect` reads it as a connect stream. */
class ReplayAgent extends HttpAgent {
  constructor(body) {
    super({
      url: "http://replay.invalid/agui",
      fetch: async () =>
        new Response(body, { status: 200, headers: { "content-type": "text/event-stream" } }),
    });
  }

  // `connect` is the hook `connectAgent` reads its stream from; its default throws. The transport
  // stages are the ones `HttpAgent.run` uses, so the connect stream is parsed exactly like a run.
  connect(input) {
    return transformHttpEventStream(
      runHttpRequest(() => this.fetch(this.url, this.requestInit(input))),
    );
  }
}

/** Runs `fn` with console.warn and console.error captured; returns the result and what was said. */
async function capturingWarnings(fn) {
  const said = [];
  const { warn, error } = console;
  console.warn = (...args) => said.push(`warn: ${args.join(" ")}`);
  console.error = (...args) => said.push(`error: ${args.join(" ")}`);
  try {
    return { result: await fn(), said };
  } finally {
    console.warn = warn;
    console.error = error;
  }
}

/** What the reference client ends up holding after a stream. */
function observe(agent) {
  const seen = { runs: [], subagents: [], custom: [] };
  const subscriber = {
    onRunStartedEvent: ({ event }) => {
      seen.runs.push({ runId: event.runId, outcome: "open" });
    },
    onRunFinishedEvent: ({ event }) => {
      const run = seen.runs.find((r) => r.runId === event.runId);
      const outcome = event.outcome?.type ?? "success";
      run.outcome =
        outcome === "interrupt"
          ? `interrupt:${event.outcome.interrupts.map((i) => `${i.id}/${i.reason}`).join(",")}`
          : outcome;
      // the run's token usage (ADR 0056), as the client read it
      if (event.usage !== undefined) run.usage = event.usage;
    },
    onRunErrorEvent: ({ event }) => {
      const run = seen.runs.at(-1);
      run.outcome = `error:${event.code ?? "-"}`;
      if (event.usage !== undefined) run.usage = event.usage;
    },
    // our own events (`vymalo.usage`, `vymalo.usage_total`, ADR 0056): what the client handed on
    onCustomEvent: ({ event }) => {
      const custom = { name: event.name, value: event.value };
      if (event.subagentRunId !== undefined) custom.subagentRunId = event.subagentRunId;
      seen.custom.push(custom);
    },
    onSubagentStartedEvent: ({ event }) => {
      const sub = { id: event.subagentRunId, name: event.name };
      // the nesting the reference client reads: the subagent this one runs in
      if (event.parentSubagentRunId !== undefined) sub.parent = event.parentSubagentRunId;
      seen.subagents.push({ ...sub, end: "open" });
    },
    onSubagentFinishedEvent: ({ event }) => {
      const sub = seen.subagents.findLast((s) => s.id === event.subagentRunId);
      const outcome = event.outcome?.type ?? "success";
      sub.end =
        outcome === "suspended" ? `suspended:${(event.outcome.interruptIds ?? []).join(",")}` : outcome;
      if (event.result !== undefined) sub.result = event.result;
    },
    onSubagentErrorEvent: ({ event }) => {
      const sub = seen.subagents.findLast((s) => s.id === event.subagentRunId);
      sub.end = `error:${event.code ?? "-"}`;
    },
  };
  return {
    subscriber,
    summary: () => ({
      runs: seen.runs,
      subagents: seen.subagents,
      // only a stream that has some says it, so the goldens without any read as they did
      ...(seen.custom.length > 0 ? { custom: seen.custom } : {}),
      messages: agent.messages.map((m) => {
        const base = { id: m.id, role: m.role };
        if (m.name !== undefined) base.name = m.name;
        if (m.subagentRunId !== undefined) base.subagentRunId = m.subagentRunId;
        if (m.role === "activity") return { ...base, activityType: m.activityType, content: m.content };
        return { ...base, text: typeof m.content === "string" ? m.content : JSON.stringify(m.content) };
      }),
      state: agent.state,
      pendingInterrupts: agent.pendingInterrupts.map((i) => ({
        id: i.id,
        reason: i.reason,
        message: i.message,
        subagentRunId: i.subagentRunId,
      })),
    }),
  };
}

/** Reads `frames` as a connect stream. Throws on an error or a warning. */
async function connect(frames) {
  const agent = new ReplayAgent(sseBody(frames));
  const watch = observe(agent);
  const { said } = await capturingWarnings(() => agent.connectAgent({}, watch.subscriber));
  if (said.length > 0) throw new Error(`the reference client warned:\n  ${said.join("\n  ")}`);
  return watch.summary();
}

/** Reads one run as a run response. Throws on an error or a warning. */
async function runOnce(frames) {
  const agent = new ReplayAgent(sseBody(frames, { keepalives: false }));
  const watch = observe(agent);
  const runId = frames[0].event.runId;
  const { said } = await capturingWarnings(() => agent.runAgent({ runId }, watch.subscriber));
  if (said.length > 0) throw new Error(`the reference client warned:\n  ${said.join("\n  ")}`);
  return watch.summary();
}

/** Splits a stream at its RUN_STARTED events. */
function runsOf(frames) {
  const runs = [];
  for (const frame of frames) {
    if (frame.event.type === "RUN_STARTED") runs.push([]);
    if (runs.length === 0) throw new Error(`the stream starts with ${frame.event.type}, not RUN_STARTED`);
    runs.at(-1).push(frame);
  }
  return runs;
}

/** The shape of a golden, before the client sees it: frames with increasing integer ids. */
function checkShape(name, frames) {
  if (!Array.isArray(frames) || frames.length === 0) throw new Error(`${name}: not a non-empty array`);
  let last = 0;
  for (const [i, frame] of frames.entries()) {
    const keys = Object.keys(frame).sort().join(",");
    if (!["event", "event,id"].includes(keys)) throw new Error(`${name}[${i}]: keys are ${keys}`);
    if (typeof frame.event?.type !== "string") throw new Error(`${name}[${i}]: no event type`);
    if (frame.id !== undefined) {
      if (!Number.isInteger(frame.id) || frame.id <= last) {
        throw new Error(`${name}[${i}]: id ${frame.id} does not increase (after ${last})`);
      }
      last = frame.id;
    }
  }
}

function sameJson(a, b) {
  return JSON.stringify(a) === JSON.stringify(b);
}

// ---- self-test: the harness must be able to say no ----------------------------------------

const started = (runId = "r1") => ({
  event: { type: "RUN_STARTED", threadId: "t", runId, protocolVersion: "1.0" },
});
const finished = (runId = "r1") => ({
  event: { type: "RUN_FINISHED", threadId: "t", runId, outcome: { type: "success" } },
});

const broken = {
  "an event before RUN_STARTED": [
    { event: { type: "STATE_SNAPSHOT", snapshot: {} } },
    started(),
    finished(),
  ],
  "a text message still open at RUN_FINISHED": [
    started(),
    { event: { type: "TEXT_MESSAGE_START", messageId: "m", role: "assistant" } },
    finished(),
  ],
  "a subagent still open at RUN_FINISHED": [
    started(),
    { event: { type: "SUBAGENT_STARTED", subagentRunId: "s", name: "a" } },
    finished(),
  ],
  "a run started inside a run": [started("r1"), started("r2")],
  "an event after the run ended": [
    started(),
    finished(),
    { event: { type: "STATE_SNAPSHOT", snapshot: {} } },
  ],
  "a member the schema does not know (stripped with a warning)": [
    started(),
    { event: { type: "STATE_SNAPSHOT", snapshot: {}, surprise: 1 } },
    finished(),
  ],
};

async function selfTest() {
  for (const [what, frames] of Object.entries(broken)) {
    let failed = false;
    try {
      await connect(frames);
    } catch {
      failed = true;
    }
    if (!failed) throw new Error(`self-test: the reference client accepted ${what}`);
  }
  // And it accepts a minimal stream, so the failures above are the streams' doing.
  await connect([started(), finished()]);
  console.log(`self-test: ${Object.keys(broken).length} broken streams rejected, a minimal one accepted`);
}

// ---- the goldens --------------------------------------------------------------------------

async function checkGolden(file) {
  const name = file.replace(/\.agui\.json$/, "");
  const frames = JSON.parse(await readFile(path.join(goldenDir, file), "utf8"));
  checkShape(file, frames);

  const whole = await connect(frames);
  const open = whole.runs.filter((r) => r.outcome === "open");
  if (open.length > 0) throw new Error(`${file}: runs left open: ${open.map((r) => r.runId)}`);
  const stillOpen = whole.subagents.filter((s) => s.end === "open");
  if (stillOpen.length > 0) throw new Error(`${file}: subagents left open: ${stillOpen.map((s) => s.id)}`);

  const expectedPath = path.join(expectedDir, `${name}.json`);
  if (update) {
    await writeFile(expectedPath, `${JSON.stringify(whole, null, 2)}\n`);
  } else {
    let expected;
    try {
      expected = JSON.parse(await readFile(expectedPath, "utf8"));
    } catch (e) {
      throw new Error(`${file}: no expected summary (${e.message}); run \`npm run update\` and review it`);
    }
    if (!sameJson(expected, whole)) {
      throw new Error(
        `${file}: what the client holds differs from expected/${name}.json\n--- expected\n${JSON.stringify(expected, null, 2)}\n--- got\n${JSON.stringify(whole, null, 2)}\nrun \`npm run update\` if the change is intended`,
      );
    }
  }

  const runs = runsOf(frames);
  for (const [i, run] of runs.entries()) {
    const alone = await runOnce(run);
    const outcome = alone.runs.at(-1).outcome;
    if (outcome !== whole.runs[i].outcome) {
      throw new Error(`${file}: run ${i} alone ends ${outcome}, in the stream ${whole.runs[i].outcome}`);
    }
  }
  console.log(`ok  ${file}  ${frames.length} frames, ${runs.length} run(s): ${whole.runs.map((r) => r.outcome).join(" | ")}`);
  return name;
}

async function main() {
  await selfTest();
  const files = (await readdir(goldenDir)).filter((f) => f.endsWith(".agui.json")).sort();
  if (files.length === 0) throw new Error(`no *.agui.json in ${goldenDir}`);
  const checked = [];
  for (const file of files) checked.push(await checkGolden(file));
  if (!update) {
    const stale = (await readdir(expectedDir))
      .filter((f) => f.endsWith(".json"))
      .filter((f) => !checked.includes(f.replace(/\.json$/, "")));
    if (stale.length > 0) throw new Error(`expected summaries without a golden: ${stale.join(", ")}`);
  }
  console.log(`${files.length} AG-UI goldens pass @ag-ui/client 1.0.0`);
}

main().catch((e) => {
  console.error(e.stack ?? e);
  process.exit(1);
});
