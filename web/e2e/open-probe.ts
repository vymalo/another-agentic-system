import type { Page } from "@playwright/test";
import { BASE_URL, MOCK_URL } from "./helpers";

/*
 * What a long thread costs to open, measured from outside the app (ADR 0059, slice 0): nothing in the app is
 * instrumented. The page side is an init script that samples the viewport on every animation frame, and the
 * Chrome DevTools Protocol says when the bytes of the connect stream arrived and how the main thread spent its time.
 * Every time is in milliseconds since the navigation started.
 */

/** One animation frame as the page saw it. */
export type ProbeFrame = {
  /** `performance.now()` of the frame. */
  t: number;
  /** The viewport's `scrollTop`, `scrollHeight` and `clientHeight` (-1 until the viewport is mounted). */
  top: number;
  height: number;
  client: number;
  /** How many messages of the person are in the DOM. */
  turns: number;
};

/** What the page side recorded. */
export type ProbeData = {
  /** `performance.timeOrigin`: the navigation start, in epoch milliseconds. */
  origin: number;
  frames: ProbeFrame[];
  /** Every `scroll` event of any element, with the element's `scrollTop`. */
  scrolls: { t: number; top: number }[];
  longTasks: { t: number; duration: number }[];
  /** The first frame in which a turn of the conversation is painted: in the DOM, not hidden, inside the viewport. */
  firstSeen: { t: number; top: number; height: number; client: number; turns: number } | null;
};

declare global {
  interface Window {
    __openProbe?: ProbeData;
  }
}

/** The viewport of the transcript and the messages of the person in it (`thread.aui.tsx`). */
const VIEWPORT = '[data-slot="aui_thread-viewport"]';
const USER_MESSAGE = '[data-slot="user-message"]';
/** A turn of the conversation: what the person said or what an agent did. */
const TURN = '[data-slot="user-message"], [data-slot="agent-turn"]';

/** Starts the page side in every document the page loads. Call it before the navigation. */
export function installProbe(page: Page): Promise<void> {
  return page.addInitScript(
    ({ viewport, userMessage, turn }) => {
      const data: ProbeData = {
        origin: performance.timeOrigin,
        frames: [],
        scrolls: [],
        longTasks: [],
        firstSeen: null,
      };
      window.__openProbe = data;

      // a scroll event does not bubble, but a capturing listener on the document hears every element's
      document.addEventListener(
        "scroll",
        (e) => {
          const el = e.target instanceof Element ? e.target : document.scrollingElement;
          data.scrolls.push({ t: performance.now(), top: el?.scrollTop ?? 0 });
        },
        { capture: true, passive: true },
      );
      try {
        new PerformanceObserver((list) => {
          for (const e of list.getEntries()) {
            data.longTasks.push({ t: e.startTime, duration: e.duration });
          }
        }).observe({ type: "longtask", buffered: true });
      } catch {
        // no long task entries in this browser: the count stays empty
      }

      /** A turn at either end of the transcript is painted: not hidden, inside the viewport. */
      const painted = (vp: Element): boolean => {
        const box = vp.getBoundingClientRect();
        const turns = vp.querySelectorAll(turn);
        for (const el of [turns[0], turns[turns.length - 1]]) {
          if (!el) continue;
          const r = el.getBoundingClientRect();
          const inside = r.height > 0 && r.bottom > box.top && r.top < box.bottom;
          const shown = el.checkVisibility({ checkOpacity: true, checkVisibilityCSS: true });
          if (inside && shown) return true;
        }
        return false;
      };

      const sample = () => {
        const vp = document.querySelector(viewport);
        const frame: ProbeFrame = {
          t: performance.now(),
          top: vp ? vp.scrollTop : -1,
          height: vp ? vp.scrollHeight : -1,
          client: vp ? vp.clientHeight : -1,
          turns: document.querySelectorAll(userMessage).length,
        };
        data.frames.push(frame);
        if (vp && !data.firstSeen && painted(vp)) {
          data.firstSeen = {
            t: frame.t,
            top: frame.top,
            height: frame.height,
            client: frame.client,
            turns: frame.turns,
          };
        }
        requestAnimationFrame(sample);
      };
      requestAnimationFrame(sample);
    },
    { viewport: VIEWPORT, userMessage: USER_MESSAGE, turn: TURN },
  );
}

/** How the connect stream arrived, from the protocol's network events. */
export type StreamTimes = {
  /** The request left. */
  start: number;
  /** The first bytes of the response. */
  firstByte: number;
  /** The last bytes that came while the page was measured: the end of the replay of a thread that is not running. */
  lastByte: number;
  bytes: number;
  chunks: number;
};

/** What the main thread spent, from `Performance.getMetrics` (seconds in the protocol, milliseconds here). */
export type MainThread = {
  script: number;
  layout: number;
  style: number;
  task: number;
  layouts: number;
  styleRecalcs: number;
};

/** The measure of one open. */
export type OpenMetrics = {
  turns: number;
  stream: StreamTimes | null;
  mainThread: MainThread;
  /** The first frame a turn is painted, how far from the end of the transcript it is then, and how many are in. */
  firstPaint: number | null;
  fromBottomAtFirstPaint: number | null;
  turnsAtFirstPaint: number | null;
  /** The first frame in which a message of the person is in the DOM, and the first in which every one is. */
  firstTurnInDom: number | null;
  allTurnsInDom: number | null;
  /** From this frame on, after the first paint, the viewport stays at the end of the transcript (within 1 px) for the rest of the measure. */
  finalScroll: number | null;
  /** After the first paint: scroll events, frames whose `scrollTop` differs from the frame before, the biggest such step. */
  scrollEvents: number;
  scrollJumps: number;
  largestStep: number;
  /** The distance the viewport travelled after the first paint, in px. */
  travelled: number;
  /** The same three from the first frame that has a message of the person in the DOM, painted or not. */
  scrollEventsSinceFirstTurn: number;
  scrollJumpsSinceFirstTurn: number;
  travelledSinceFirstTurn: number;
  /** The app's own marks on the way to showing the transcript (`performance.mark("open:…")`), in milliseconds since the navigation started. */
  marks: Record<string, number>;
  longTasks: number;
  blocking: number;
  frames: number;
};

const median = (xs: number[]): number => {
  const sorted = [...xs].sort((a, b) => a - b);
  const mid = Math.floor(sorted.length / 2);
  return sorted.length % 2 ? (sorted[mid] ?? 0) : ((sorted[mid - 1] ?? 0) + (sorted[mid] ?? 0)) / 2;
};

/** How the page's frames read: see `OpenMetrics`. Pure, so a test can feed it frames. */
export function summarize(
  data: ProbeData,
  expectedTurns: number,
): Omit<OpenMetrics, "turns" | "stream" | "mainThread" | "marks"> {
  const { frames } = data;
  const first = data.firstSeen;
  const after = first ? frames.filter((f) => f.t >= first.t) : [];
  const firstTurn = frames.find((f) => f.turns > 0);
  const since = firstTurn ? frames.filter((f) => f.t >= firstTurn.t) : [];
  const all = frames.find((f) => f.turns >= expectedTurns);
  // the last frame after the first paint that was not at the end of the transcript: the position is final from the next one on
  const lastAway = after.findLastIndex((f) => f.height - f.client - f.top > 1);
  const final = after[lastAway + 1];
  const painted = movement(after);
  const replayed = movement(since);
  return {
    firstPaint: first ? first.t : null,
    fromBottomAtFirstPaint: first ? first.height - first.client - first.top : null,
    turnsAtFirstPaint: first ? first.turns : null,
    firstTurnInDom: firstTurn ? firstTurn.t : null,
    allTurnsInDom: all ? all.t : null,
    finalScroll: final ? final.t : null,
    scrollEvents: first ? data.scrolls.filter((s) => s.t >= first.t).length : 0,
    scrollJumps: painted.jumps,
    largestStep: painted.largest,
    travelled: painted.travelled,
    scrollEventsSinceFirstTurn: firstTurn
      ? data.scrolls.filter((s) => s.t >= firstTurn.t).length
      : 0,
    scrollJumpsSinceFirstTurn: replayed.jumps,
    travelledSinceFirstTurn: replayed.travelled,
    longTasks: data.longTasks.length,
    blocking: data.longTasks.reduce((sum, t) => sum + Math.max(0, t.duration - 50), 0),
    frames: frames.length,
  };
}

/** How the viewport moved across `frames`: the frames whose `scrollTop` differs from the frame before, the biggest step and the sum. */
function movement(frames: ProbeFrame[]): { jumps: number; largest: number; travelled: number } {
  let jumps = 0;
  let largest = 0;
  let travelled = 0;
  for (let i = 1; i < frames.length; i++) {
    const step = Math.abs((frames[i]?.top ?? 0) - (frames[i - 1]?.top ?? 0));
    if (step > 0) jumps++;
    largest = Math.max(largest, step);
    travelled += step;
  }
  return { jumps, largest, travelled };
}

/**
 * Opens `threadId` in `page` and measures it until every one of `turns` messages is in the page, plus `tail`
 * milliseconds (a late scroll or a late layout shows up in the tail). `throttle` slows the CPU by that factor.
 */
export async function measureOpen(
  page: Page,
  threadId: string,
  turns: number,
  options: { tail?: number; throttle?: number; search?: string } = {},
): Promise<OpenMetrics> {
  const { tail = 1500, throttle = 1, search = "" } = options;
  const cdp = await page.context().newCDPSession(page);
  await cdp.send("Network.enable");
  await cdp.send("Performance.enable");
  if (throttle > 1) await cdp.send("Emulation.setCPUThrottlingRate", { rate: throttle });

  // the connect stream, on the protocol's clock; `wall` pins that clock to the epoch the page's clock is on
  let connectId: string | undefined;
  let anchor: { wall: number; ts: number } | undefined;
  const received: { ts: number; bytes: number }[] = [];
  cdp.on("Network.requestWillBeSent", (e) => {
    if (e.request.url.includes(`/agui/threads/${threadId}/connect`) && !connectId) {
      connectId = e.requestId;
      anchor = { wall: e.wallTime, ts: e.timestamp };
    }
  });
  cdp.on("Network.dataReceived", (e) => {
    if (e.requestId === connectId) received.push({ ts: e.timestamp, bytes: e.dataLength });
  });

  await page.goto(`${BASE_URL}/threads/${threadId}${search}`, { waitUntil: "commit" });
  await page.waitForFunction((n) => (window.__openProbe?.frames.at(-1)?.turns ?? 0) >= n, turns, {
    timeout: 900_000,
  });
  await page.waitForTimeout(tail);

  const data = await page.evaluate(() => window.__openProbe as ProbeData);
  const raw = (await cdp.send("Performance.getMetrics")).metrics;
  const metric = (name: string): number => raw.find((m) => m.name === name)?.value ?? 0;
  const mainThread: MainThread = {
    script: metric("ScriptDuration") * 1000,
    layout: metric("LayoutDuration") * 1000,
    style: metric("RecalcStyleDuration") * 1000,
    task: metric("TaskDuration") * 1000,
    layouts: metric("LayoutCount"),
    styleRecalcs: metric("RecalcStyleCount"),
  };
  if (throttle > 1) await cdp.send("Emulation.setCPUThrottlingRate", { rate: 1 });
  await cdp.detach();

  const since = (ts: number): number =>
    anchor ? (anchor.wall + (ts - anchor.ts)) * 1000 - data.origin : Number.NaN;
  const stream: StreamTimes | null =
    anchor && received.length > 0
      ? {
          start: since(anchor.ts),
          firstByte: since(received[0]?.ts ?? 0),
          lastByte: since(received.at(-1)?.ts ?? 0),
          bytes: received.reduce((sum, r) => sum + r.bytes, 0),
          chunks: received.length,
        }
      : null;
  const marks = await page.evaluate(() =>
    Object.fromEntries(
      performance
        .getEntriesByType("mark")
        .filter((m) => m.name.startsWith("open:"))
        .map((m) => [m.name, m.startTime]),
    ),
  );
  return { turns, stream, mainThread, marks, ...summarize(data, turns) };
}

/** A finished thread of `turns` turns, made by the mock (`POST /__mock/long-thread`). */
export async function seedLongThread(turns: number): Promise<string> {
  const res = await fetch(`${MOCK_URL}/__mock/long-thread?turns=${turns}`, { method: "POST" });
  if (res.status !== 201) throw new Error(`the mock made no long thread: ${res.status}`);
  return ((await res.json()) as { threadId: string }).threadId;
}

/** The median of each number of a list of measures (a list of one is itself). */
export function medianOf(runs: OpenMetrics[]): Record<string, number | null> {
  const pick = (f: (m: OpenMetrics) => number | null): number | null => {
    const xs = runs.map(f).filter((x): x is number => x !== null && Number.isFinite(x));
    return xs.length === runs.length ? Math.round(median(xs) * 10) / 10 : null;
  };
  const marks = [...new Set(runs.flatMap((m) => Object.keys(m.marks)))].sort();
  return {
    ...Object.fromEntries(marks.map((name) => [name, pick((m) => m.marks[name] ?? null)])),
    streamFirstByte: pick((m) => m.stream?.firstByte ?? null),
    streamLastByte: pick((m) => m.stream?.lastByte ?? null),
    streamKiB: pick((m) => (m.stream ? m.stream.bytes / 1024 : null)),
    firstPaint: pick((m) => m.firstPaint),
    fromBottomAtFirstPaint: pick((m) => m.fromBottomAtFirstPaint),
    turnsAtFirstPaint: pick((m) => m.turnsAtFirstPaint),
    firstTurnInDom: pick((m) => m.firstTurnInDom),
    allTurnsInDom: pick((m) => m.allTurnsInDom),
    finalScroll: pick((m) => m.finalScroll),
    scrollEvents: pick((m) => m.scrollEvents),
    scrollJumps: pick((m) => m.scrollJumps),
    largestStep: pick((m) => m.largestStep),
    travelled: pick((m) => m.travelled),
    scrollEventsSinceFirstTurn: pick((m) => m.scrollEventsSinceFirstTurn),
    scrollJumpsSinceFirstTurn: pick((m) => m.scrollJumpsSinceFirstTurn),
    travelledSinceFirstTurn: pick((m) => m.travelledSinceFirstTurn),
    longTasks: pick((m) => m.longTasks),
    blockingMs: pick((m) => m.blocking),
    scriptMs: pick((m) => m.mainThread.script),
    layoutMs: pick((m) => m.mainThread.layout),
    styleMs: pick((m) => m.mainThread.style),
    taskMs: pick((m) => m.mainThread.task),
    layouts: pick((m) => m.mainThread.layouts),
    styleRecalcs: pick((m) => m.mainThread.styleRecalcs),
  };
}
