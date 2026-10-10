// @vitest-environment jsdom

import { readFileSync } from "node:fs";
import path from "node:path";
import { cleanup, configure } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { asRepository, buildMessages, joinMessages } from "./seed";
import { GOLDEN_DIR, type GoldenFrame, THREAD_ID } from "./testing";
import { replay, seedRuns, stable } from "./testing-history";
import { summarize } from "./testing-runtime";

configure({ asyncUtilTimeout: 10_000 });
afterEach(cleanup);

/**
 * Pages of a thread's history put together over a walk of several pages (ADR 0059, docs/api/history.md rule 1), in the
 * browser: the reader opens the newest page and scrolls up through the others one at a time, and after each page the
 * messages it holds are the messages a replay of the frames it holds shows. `windowed.dom.test.tsx` cuts a golden in two;
 * these are the shapes that only a longer walk shows: a surface that is made in one turn and changed in the next, a message
 * sent while the agent works followed by a turn of its own, and a question that a later turn answers or that nobody has.
 * The pages are the ones the orchestrator writes for logs built by `orch-agui-projection`'s `tests/carry.rs`
 * (docs/api/examples/history/*.walk.json), one turn to a page, newest first.
 */

type Page = { frames: GoldenFrame[] };

const walk = (name: string): Page[] =>
  (
    JSON.parse(
      readFileSync(path.join(GOLDEN_DIR, "..", "history", `${name}.walk.json`), "utf8"),
    ) as { pages: Page[] }
  ).pages;

type Held = Awaited<ReturnType<typeof buildMessages>>;

/** The reader holding the newest page, then each older one in front, after every one of which `check` looks. */
async function scrollUp(
  pages: Page[],
  check: (held: Held, frames: GoldenFrame[], page: number) => Promise<void>,
) {
  let held = await buildMessages(await seedRuns(pages[0]?.frames ?? []), THREAD_ID);
  let frames = [...(pages[0]?.frames ?? [])];
  await check(held, frames, 0);
  for (let k = 1; k < pages.length; k++) {
    const older = await buildMessages(await seedRuns(pages[k]?.frames ?? []), THREAD_ID, true);
    held = joinMessages(older, held);
    frames = [...(pages[k]?.frames ?? []), ...frames];
    await check(held, frames, k);
  }
}

async function equalsReplay(held: Held, frames: GoldenFrame[], page: number) {
  const replayed = await replay(frames);
  // what an import takes: a chain of the messages, each once
  expect(new Set(held.map((m) => m.id)).size).toBe(held.length);
  expect(asRepository(held).messages).toHaveLength(held.length);
  expect(summarize(held), `holding ${page + 1} pages`).toEqual(summarize(replayed));
  expect(stable(held), `holding ${page + 1} pages`).toEqual(stable(replayed));
}

describe("a walk of several pages is the replay of the pages held", () => {
  for (const name of ["surface-turns", "steer-turns", "form-turns"]) {
    it(`${name}: after every page`, async () => {
      const pages = walk(name);
      expect(pages.length).toBeGreaterThanOrEqual(5);
      await scrollUp(pages, equalsReplay);
    }, 120_000);
  }

  it("surface-turns: a surface that spans turns is drawn in every turn that changed it, and in none that did not", async () => {
    const pages = walk("surface-turns");
    let held: Held = [];
    await scrollUp(pages, async (messages) => {
      held = messages;
    });
    // `s0` is changed in all five turns and `s1` made in the fourth and changed in the fifth: the surfaces each turn's message holds
    const surfaces = summarize(held)
      .filter((m) => m.role === "assistant")
      .map((m) => m.parts.filter((p) => p === "a2ui-surface").length);
    expect(surfaces).toEqual([1, 1, 1, 2, 2]);
  }, 120_000);

  it("steer-turns: a steered turn keeps the person's messages in the order they were sent", async () => {
    const pages = walk("steer-turns");
    let held: Held = [];
    await scrollUp(pages, async (messages) => {
      held = messages;
    });
    const said = summarize(held)
      .filter((m) => m.role === "user")
      .flatMap((m) => m.parts);
    expect(said).toEqual([
      "text:turn 1",
      "text:and this too",
      "text:turn 2",
      "text:turn 3",
      "text:also this",
      "text:and then that",
      "text:turn 4",
      "text:turn 5",
    ]);
  }, 120_000);

  it("form-turns: the questions that a later turn answered are settled, and the newest still waits", async () => {
    const pages = walk("form-turns");
    let held: Held = [];
    await scrollUp(pages, async (messages) => {
      held = messages;
    });
    const answers = summarize(held)
      .filter((m) => m.role === "assistant")
      .map((m) => m.status);
    expect(answers.at(-1)).toBe("requires-action:interrupt");
    expect(answers.slice(0, -1).every((s) => s !== "requires-action:interrupt")).toBe(true);
    expect(answers).toHaveLength(5);
  }, 120_000);
});
