import { expect, test } from "@playwright/test";
import {
  installProbe,
  measureOpen,
  medianOf,
  type OpenMetrics,
  type ProbeData,
  seedLongThread,
  summarize,
} from "./open-probe";

/*
 * What it costs to open a long thread (ADR 0059, slice 0): the web's mock makes a finished thread of many turns
 * (`POST /__mock/long-thread`), a fresh browser opens it, and the numbers are the time to the first painted message,
 * the time the viewport reaches its final position, how often it moves in between, and where the time goes (the
 * stream's bytes, the main thread's script, layout and style). The spec asserts only that the thread opens and ends
 * at the bottom; the numbers are printed (`OPEN_METRICS`) and attached as `open-metrics.json`.
 *
 *   OPEN_RUNS=5 OPEN_CPU_THROTTLE=4 pnpm exec playwright test open-long-thread --project=chromium --workers=1
 *
 * `OPEN_TURNS` (default 40) is the length of the thread, `OPEN_RUNS` (default 1) how many times it is opened, each in a
 * fresh browser context, `OPEN_CPU_THROTTLE` (default 1) the factor the CPU is slowed by and `OPEN_SEARCH` (default
 * none) what follows the thread's address, a flag in the query, say. A thread of 200 turns takes minutes to open while
 * the replay is what opens it (the cost grows with the square of the turns). Run the spec alone: other specs on the
 * machine move the numbers.
 */

const TURNS = Number(process.env.OPEN_TURNS ?? 40);
const RUNS = Number(process.env.OPEN_RUNS ?? 1);
const THROTTLE = Number(process.env.OPEN_CPU_THROTTLE ?? 1);
const SEARCH = process.env.OPEN_SEARCH ?? "";

test("records what it costs to open a thread of many turns", async ({ browser }, info) => {
  test.setTimeout(Math.max(120_000, TURNS * 5_000) * RUNS);
  const id = await seedLongThread(TURNS);
  const runs: OpenMetrics[] = [];
  for (let i = 0; i < RUNS; i++) {
    const context = await browser.newContext();
    const page = await context.newPage();
    await installProbe(page);
    runs.push(await measureOpen(page, id, TURNS, { throttle: THROTTLE, search: SEARCH }));
    await context.close();
  }
  const report = { turns: TURNS, throttle: THROTTLE, search: SEARCH, median: medianOf(runs), runs };
  console.log(`OPEN_METRICS ${JSON.stringify(report.median)}`);
  await info.attach("open-metrics.json", {
    body: JSON.stringify(report, null, 2),
    contentType: "application/json",
  });
  // it opens, and it ends where the end of the conversation is
  for (const run of runs) {
    expect(run.finalScroll, "the viewport reached the end of the transcript").not.toBeNull();
  }
});

/** A frame of the sampler: `top` of a viewport `client` tall that holds `height` of content. */
const frame = (t: number, top: number, turns = 1, height = 5000, client = 800) => ({
  t,
  top,
  height,
  client,
  turns,
});

const data = (frames: ProbeData["frames"], scrolls: number[], seenAt?: number): ProbeData => {
  const seen = frames.find((f) => f.t === seenAt);
  return {
    origin: 0,
    frames,
    scrolls: scrolls.map((t) => ({ t, top: 0 })),
    longTasks: [],
    firstSeen: seen
      ? { t: seen.t, top: seen.top, height: seen.height, client: seen.client, turns: seen.turns }
      : null,
  };
};

test.describe("the probe reads frames", () => {
  test("a transcript that opens at its end and stays there has no step after the first paint", () => {
    const frames = [frame(10, -1, 0), frame(20, 4200, 3), frame(30, 4200, 3), frame(40, 4200, 3)];
    const m = summarize(data(frames, [], 20), 3);
    expect(m.firstPaint).toBe(20);
    expect(m.fromBottomAtFirstPaint).toBe(0);
    expect(m.finalScroll).toBe(20);
    expect(m.scrollJumps).toBe(0);
    expect(m.scrollEvents).toBe(0);
  });

  test("a transcript that is scrolled while it fills counts every frame that moved", () => {
    const frames = [
      frame(10, 0, 1, 900),
      frame(20, 0, 2, 1800),
      frame(30, 400, 2, 1800),
      frame(40, 700, 3, 2700),
      frame(50, 1900, 3, 2700),
    ];
    const m = summarize(data(frames, [30, 40, 50], 10), 3);
    expect(m.firstPaint).toBe(10);
    expect(m.fromBottomAtFirstPaint).toBe(100);
    expect(m.scrollJumps).toBe(3);
    expect(m.scrollEvents).toBe(3);
    expect(m.largestStep).toBe(1200);
    expect(m.travelled).toBe(1900);
    expect(m.allTurnsInDom).toBe(40);
    expect(m.finalScroll).toBe(50);
  });

  test("the movement before the first paint is counted from the first message that is in the page", () => {
    // frames 20 and 30 have a message in the DOM and nothing painted; the viewport moves in them
    const frames = [
      frame(10, 0, 0, 800),
      frame(20, 0, 1, 1800),
      frame(30, 500, 2, 2800),
      frame(40, 2000, 3, 2800),
      frame(50, 2000, 3, 2800),
    ];
    const m = summarize(data(frames, [25, 30], 40), 3);
    expect(m.firstTurnInDom).toBe(20);
    expect(m.firstPaint).toBe(40);
    expect(m.scrollJumps).toBe(0);
    expect(m.scrollJumpsSinceFirstTurn).toBe(2);
    expect(m.travelledSinceFirstTurn).toBe(2000);
    expect(m.scrollEventsSinceFirstTurn).toBe(2);
    expect(m.scrollEvents).toBe(0);
  });

  test("a viewport that is away from the end in the last frame has no final position", () => {
    const m = summarize(data([frame(10, 0), frame(20, 100)], [], 10), 1);
    expect(m.finalScroll).toBeNull();
  });
});
