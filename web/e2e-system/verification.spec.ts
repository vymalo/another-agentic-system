import { expect, test } from "@playwright/test";
import { badge, callsFor, framesOf, resetDb, shape, startThread, threadId } from "./helpers";

/*
 * The verification gate (ADR 0018) through the real orchestrator: `gated` is the `plain` fake agent
 * under `gate: {require: [agent-checks]}` (agents.yaml), and its `verify-*` scripts push a commit and
 * report checks that fail once (`verify-red-once`) or always (`verify-red`). The states go by faster
 * than a browser can watch here; the dom tests pin them one by one.
 */

test.beforeEach(resetDb);

const log = (page: import("@playwright/test").Page) =>
  page.getByRole("log", { name: "Conversation" });
const checks = (page: import("@playwright/test").Page) =>
  log(page).getByRole("region", { name: /^Check: / });

test("red once: sent back with the finding, done on attempt 2 of 3", async ({ page }) => {
  await startThread(page, "verify-red-once fix the login", "Gated");

  await expect(badge(page)).toHaveText("Done");
  await expect(checks(page)).toHaveCount(2);
  const failed = log(page).getByRole("region", { name: "Check: Agent checks, attempt 1, failed" });
  await expect(failed.getByText("tests::login fails: expected 200, got 500")).toBeVisible();
  await expect(
    log(page).getByRole("region", { name: "Check: Agent checks, attempt 2, passed" }),
  ).toBeVisible();
  await expect(log(page).getByText("Attempt 2 of 3: sent back with 1 finding")).toBeVisible();

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
  await expect(checks(page)).toHaveCount(3);
  await expect(page.getByLabel("Message")).toBeEnabled();

  const frames = shape(await framesOf(page.request, threadId(page)));
  expect(frames.filter((f) => f === "ACTIVITY_SNAPSHOT:vymalo.rework")).toHaveLength(2);
  expect(frames.at(-1)).toBe("RUN_ERROR:checks_failed");
});

test("green the first time: there is nothing to rework", async ({ page }) => {
  await startThread(page, "verify-pass fix the login", "Gated");

  await expect(badge(page)).toHaveText("Done");
  await expect(checks(page)).toHaveCount(1);
  await expect(log(page).getByText(/sent back with/)).toHaveCount(0);
});

test("an agent without a gate has no cards", async ({ page }) => {
  await startThread(page, "echo hello", "Plain");

  await expect(badge(page)).toHaveText("Done");
  await expect(checks(page)).toHaveCount(0);
});
