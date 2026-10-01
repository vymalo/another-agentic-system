import { expect, test } from "@playwright/test";
import {
  activityTab,
  badge,
  expectNoHorizontalScroll,
  hideActivity,
  showActivity,
  startThread,
} from "./helpers";

/*
 * CI results (ADR 0017), against the mock, which plays the golden `ci` of docs/api/examples for
 * `verify-ci`. `Reviewer` is the agent without releases. A report is a step of the turn, so it is the
 * panel's Activity tab (on a phone a sheet: opened with `showActivity`, closed to read the page).
 */

const reports = (page: import("@playwright/test").Page) =>
  activityTab(page).getByRole("listitem", { name: /^CI: / });

test("a red report sends the agent back, a green one finishes the job; each report is a card", async ({
  page,
}) => {
  test.setTimeout(60_000);
  await startThread(page, "verify-ci fix the login", "Reviewer");

  const tab = await showActivity(page);
  const red = tab.getByRole("listitem", { name: "CI: ci/build, failure" });
  await expect(red).toBeVisible();
  await expect(red.getByText("Failure", { exact: true })).toBeVisible();
  await expect(red.getByText("1 test failed: tests::login")).toBeVisible();
  await expect(red.getByText("agent/fix")).toBeVisible();
  await expect(red.getByText("0000000")).toHaveAttribute("title", /0001$/);
  const link = red.getByRole("link", { name: /View run/ });
  await expect(link).toHaveAttribute("href", "https://ci.example.com/runs/1");
  await expect(link).toHaveAttribute("target", "_blank");
  await expect(link).toHaveAttribute("rel", "noopener noreferrer");

  await expect(reports(page)).toHaveCount(2, { timeout: 30_000 });
  const green = tab.getByRole("listitem", { name: "CI: ci/build, success" });
  await expect(green.getByText("3 tests passed")).toBeVisible();
  await expect(tab.getByText("CI failed — trying again (2/3)")).toBeVisible();
  await hideActivity(page);
  await expect(badge(page)).toHaveText("Done", { timeout: 30_000 });
  await expectNoHorizontalScroll(page);

  // a reload is a replay: the same two cards
  await page.reload();
  await expect(badge(page)).toHaveText("Done");
  await showActivity(page);
  await expect(reports(page)).toHaveCount(2);
});

test("a report of an older push is a card of its own next to the current one", async ({ page }) => {
  await startThread(page, "verify-ci-stale ship it", "Reviewer");
  await expect(badge(page)).toHaveText("Done", { timeout: 30_000 });
  await showActivity(page);
  await expect(reports(page)).toHaveCount(2);
  await expect(reports(page).nth(0)).toHaveAttribute("data-conclusion", "failure");
  await expect(reports(page).nth(1)).toHaveAttribute("data-conclusion", "success");
  await expect(activityTab(page).getByText("build passed").first()).toBeVisible();
  await hideActivity(page);
  await expectNoHorizontalScroll(page);
});

test("a thread that waits for CI has no report card yet", async ({ page }) => {
  await startThread(page, "verify-wait ship it", "Reviewer");
  await expect(badge(page)).toHaveText("Checking the work…");
  await showActivity(page);
  await expect(reports(page)).toHaveCount(0);
});
