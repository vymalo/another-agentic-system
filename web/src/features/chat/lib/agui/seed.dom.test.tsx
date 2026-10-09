// @vitest-environment jsdom

import { cleanup, configure } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { buildMessages } from "./seed";
import { loadGolden, THREAD_ID } from "./testing";
import { GOLDENS, replay, seedRuns, settled, stable } from "./testing-history";
import { summarize } from "./testing-runtime";

configure({ asyncUtilTimeout: 10_000 });
afterEach(cleanup);

/**
 * The seed (ADR 0059) against the runtime it stands in for: for every golden, the messages `buildMessages` makes from the
 * runs of a page equal the ones the visible runtime holds after the same frames as a replay of the whole log. If this
 * fails, a thread opened at its end would show something else than the same thread opened from its first event.
 */

describe("the seed equals the replay", () => {
  for (const name of GOLDENS) {
    const frames = loadGolden(name);
    if (!settled(frames)) continue;
    it(`${name}: the messages of the page are the messages of the replay`, async () => {
      const replayed = await replay(frames);
      expect(replayed.length).toBeGreaterThan(0);
      const runs = await seedRuns(frames);
      const built = await buildMessages(runs, THREAD_ID);
      expect(summarize(built)).toEqual(summarize(replayed));
      // the whole message, not only its parts' names: the ids that are the server's are the same and the rest has the same shape
      expect(stable(built)).toEqual(stable(replayed));
    }, 60_000);
  }
});
