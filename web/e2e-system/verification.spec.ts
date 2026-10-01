import { expect, test } from "@playwright/test";
import {
  activityTab,
  badge,
  callsFor,
  framesOf,
  resetDb,
  shape,
  showActivity,
  startThread,
  threadId,
} from "./helpers";

/*
 * The verification gate (ADR 0018) through the real orchestrator: `gated` is the `plain` fake agent
 * under `gate: {require: [agent-checks]}` (agents.yaml), and its `verify-*` scripts push a commit and
 * report checks that fail once (`verify-red-once`) or always (`verify-red`). The states go by faster
 * than a browser can watch here; the dom tests pin them one by one. The checks and the reworks are
 * steps of the turn, so they are the side panel's Activity tab (docked and open on this window).
 */

test.beforeEach(resetDb);

const checks = (page: import("@playwright/test").Page) =>
  activityTab(page).getByRole("listitem", { name: /^Check: / });

test("red once: sent back with the finding, done on attempt 2 of 3", async ({ page }) => {
  await startThread(page, "verify-red-once fix the login", "Gated");

  await expect(badge(page)).toHaveText("Done");
  await showActivity(page);
  await expect(checks(page)).toHaveCount(2);
  const failed = activityTab(page).getByRole("listitem", {
    name: "Check: Agent checks, attempt 1, failed",
  });
  // the findings are folded under the check: open them
  await failed.getByText("Findings (1)").click();
  await expect(failed.getByText("tests::login fails: expected 200, got 500")).toBeVisible();
  await expect(
    activityTab(page).getByRole("listitem", { name: "Check: Agent checks, attempt 2, passed" }),
  ).toBeVisible();
  await expect(activityTab(page).getByText("Checks failed — trying again (2/3)")).toBeVisible();

  // one run for both attempts, as the orchestrator projects the log
  const frames = shape(await framesOf(page.request, threadId(page)));
  expect(frames.filter((f) => f === "RUN_STARTED")).toHaveLength(1);
  expect(frames.filter((f) => f === "STATE_SNAPSHOT:verifying")).toHaveLength(2);
  expect(frames).toContain("ACTIVITY_SNAPSHOT:vymalo.check:failed");
  expect(frames).toContain("ACTIVITY_SNAPSHOT:vymalo.check:passed");
  expect(frames).toContain("ACTIVITY_SNAPSHOT:vymalo.rework");
  expect(frames.at(-1)).toBe("RUN_FINISHED:success");

  // the second attempt is a new task of the same context, sent the findings
  const first = (await callsFor(page.request, "plain", "verify-red-once fix the login"))[0];
  const second = (await callsFor(page.request, "plain", "Your work did not pass verification"))[0];
  expect(second?.contextId).toBe(first?.contextId);
  expect(second?.taskId).not.toBe(first?.taskId);
  expect(second?.text).toContain("tests::login fails: expected 200, got 500");
});

test("red every time: three attempts, then 'Checks failed after 3 attempts'", async ({ page }) => {
  await startThread(page, "verify-red fix the login", "Gated");

  await expect(badge(page)).toHaveText("Failed");
  await expect(page.getByText("Checks failed after 3 attempts")).toBeVisible();
  await expect(page.getByText("This thread is failed.")).toHaveCount(0);
  await showActivity(page);
  await expect(checks(page)).toHaveCount(3);
  await expect(page.getByLabel("Message")).toBeEnabled();

  const frames = shape(await framesOf(page.request, threadId(page)));
  expect(frames.filter((f) => f === "ACTIVITY_SNAPSHOT:vymalo.rework")).toHaveLength(2);
  expect(frames.at(-1)).toBe("RUN_ERROR:checks_failed");
});

test("green the first time: there is nothing to rework", async ({ page }) => {
  await startThread(page, "verify-pass fix the login", "Gated");

  await expect(badge(page)).toHaveText("Done");
  await showActivity(page);
  await expect(checks(page)).toHaveCount(1);
  await expect(activityTab(page).getByText(/trying again/)).toHaveCount(0);
});

test("an agent without a gate has no cards", async ({ page }) => {
  await startThread(page, "echo hello", "Plain");

  await expect(badge(page)).toHaveText("Done");
  await showActivity(page);
  await expect(checks(page)).toHaveCount(0);
});
