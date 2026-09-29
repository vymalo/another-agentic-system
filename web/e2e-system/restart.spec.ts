import { expect, test } from "@playwright/test";
import {
  badge,
  callsFor,
  ECHO,
  framesOf,
  killOrchestrator,
  releaseGate,
  resetDb,
  seqs,
  shape,
  startOrchestrator,
  startThread,
  threadId,
  waitForExecution,
} from "./helpers";

test.beforeEach(resetDb);

test("orchestrator restart mid-thread: the page recovers and finishes", async ({ page }) => {
  await startThread(page, "gate restart", "Plain");
  await waitForExecution(page.request, "plain", "gate restart");
  await expect(badge(page)).toHaveText("Working");
  const id = threadId(page);

  // a crash: no graceful shutdown, so the delegation's lease must expire (5 s here)
  await killOrchestrator();
  await expect(page.getByText("Reconnecting…")).toBeVisible();
  await startOrchestrator();
  await releaseGate(page.request, "plain");

  await expect(badge(page)).toHaveText("Done", { timeout: 45_000 });
  await expect(page.getByText("Reconnecting…")).toHaveCount(0);
  const log = page.getByRole("log", { name: "Conversation" });
  await expect(log.getByText("echo: gate restart")).toHaveCount(1);
  await expect(log.getByText("Working")).toHaveCount(1);

  const frames = await framesOf(page.request, id);
  expect(shape(frames)).toEqual(ECHO);
  expect(seqs(frames)).toEqual([1, 2, 3, 4, 5]);
  // the message reached the agent exactly once: the new process resumed the task
  const executions = (await callsFor(page.request, "plain", "gate restart")).filter(
    (c) => c.kind === "execute",
  );
  expect(executions).toHaveLength(1);
});
