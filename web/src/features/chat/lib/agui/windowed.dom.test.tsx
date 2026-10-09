// @vitest-environment jsdom

import { cleanup, configure } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { asRepository, buildMessages, joinMessages } from "./seed";
import { type GoldenFrame, loadGolden, THREAD_ID } from "./testing";
import { GOLDENS, replay, seedRuns, settled, stable } from "./testing-history";
import { summarize } from "./testing-runtime";

configure({ asyncUtilTimeout: 10_000 });
afterEach(cleanup);

/**
 * Pages of a thread's history put together are the thread (ADR 0059, docs/api/history.md rule 1), in the browser: for every
 * golden and every settled point in it, the frames before the point and the frames after it are two pages; the messages of
 * the older page joined in front of the newer page's are the messages the visible runtime holds after a replay of the whole
 * log. This is what a person sees after scrolling up through a thread that was opened at its end.
 */

/**
 * The places a log may be cut: where a run starts after an event that left none open (a chain starts; a steering message closes
 * a run and opens the next in one event, so it is no place), the frames between two chains riding along with the one before.
 */
function cuts(frames: readonly GoldenFrame[]): number[] {
  let open = 0;
  const at: number[] = [];
  frames.forEach((f, i) => {
    const type = f.event.type;
    if (type === "RUN_STARTED") {
      if (open === 0 && frames[i - 1]?.id !== undefined) at.push(i);
      open++;
    }
    if (type === "RUN_FINISHED" || type === "RUN_ERROR") open--;
  });
  return at;
}

describe("the pages of a golden are the golden", () => {
  for (const name of GOLDENS) {
    const frames = loadGolden(name);
    if (!settled(frames)) continue;
    const points = cuts(frames);
    if (points.length === 0) continue;
    it(`${name}: older page + newer page = the replay (${points.length} cut${points.length === 1 ? "" : "s"})`, async () => {
      const replayed = await replay(frames);
      for (const at of points) {
        const older = await buildMessages(await seedRuns(frames.slice(0, at)), THREAD_ID, true);
        const newer = await buildMessages(await seedRuns(frames.slice(at)), THREAD_ID);
        const joined = joinMessages(older, newer);
        // what an import takes: a chain of the messages, each once
        expect(new Set(joined.map((m) => m.id)).size).toBe(joined.length);
        expect(asRepository(joined).messages).toHaveLength(joined.length);
        expect(summarize(joined), `cut after frame ${at}`).toEqual(summarize(replayed));
        expect(stable(joined), `cut after frame ${at}`).toEqual(stable(replayed));
      }
    }, 120_000);
  }
});
