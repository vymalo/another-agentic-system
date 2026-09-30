import { expect, test } from "@playwright/test";
import { badge, conversation, expectNoHorizontalScroll, startThread } from "./helpers";

/*
 * The verification gate (ADR 0018), against the mock, which plays the goldens verify-green and
 * verify-red of docs/api/examples. `Reviewer` is the agent without releases.
 */

const counter = (page: import("@playwright/test").Page) =>
  page.locator("[data-slot='attempt-counter']");
const checks = (page: import("@playwright/test").Page) =>
  conversation(page).getByRole("region", { name: /^Check: / });

test("red once: verifying, sent back, verifying again, done; the counter and the cards follow", async ({
  page,
}) => {
  await startThread(page, "verify-red-once fix the login", "Reviewer");

  // the states go by at the mock's pace; the counter's steps are the dom tests' (1/3, then 2/3)
  await expect(counter(page)).toContainText(/Attempt [12] of 3/);
  const failed = conversation(page).getByRole("region", {
    name: "Check: Agent checks, attempt 1, failed",
  });
  await expect(failed).toBeVisible();
  await expect(failed.getByText("tests::login fails: expected 200, got 500")).toBeVisible();
  await expect(
    conversation(page).getByText("Attempt 2 of 3: sent back with 1 finding"),
  ).toBeVisible();

  await expect(badge(page)).toHaveText("Done");
  await expect(counter(page)).toContainText("Attempt 2 of 3");
  await expect(checks(page)).toHaveCount(2);
  await expect(
    conversation(page).getByRole("region", { name: "Check: Agent checks, attempt 2, passed" }),
  ).toBeVisible();
  await expectNoHorizontalScroll(page);
});

test("red every time: 'Checks failed after 3 attempts', not an ordinary failure", async ({
  page,
}) => {
  test.setTimeout(60_000);
  await startThread(page, "verify-red fix the login", "Reviewer");

  await expect(badge(page)).toHaveText("Failed", { timeout: 30_000 });
  await expect(counter(page)).toContainText("Attempt 3 of 3");
  await expect(page.getByText("Checks failed after 3 attempts")).toBeVisible();
  await expect(page.getByText("This thread is failed.")).toHaveCount(0);
  await expect(checks(page)).toHaveCount(3);
  await expect(page.getByLabel("Message")).toBeDisabled();
  await expectNoHorizontalScroll(page);
});

test("waiting for CI: a pending card while the thread is verifying; Cancel ends it; a reload shows the same", async ({
  page,
}) => {
  await startThread(page, "verify-wait ship it", "Reviewer");

  await expect(badge(page)).toHaveText("Verifying");
  await expect(
    conversation(page).getByRole("region", { name: "Check: CI, attempt 1, pending" }),
  ).toBeVisible();
  await page.reload();
  await expect(badge(page)).toHaveText("Verifying");
  await expect(counter(page)).toContainText("Attempt 1 of 3");
  await expect(checks(page)).toHaveCount(2);

  await page.getByRole("button", { name: "Cancel" }).click();
  await expect(badge(page)).toHaveText("Cancelled");
  await expect(checks(page)).toHaveCount(2);
});

test("a thread without a gate shows no counter", async ({ page }) => {
  await startThread(page, "echo hello", "Reviewer");
  await expect(badge(page)).toHaveText("Done");
  await expect(counter(page)).toHaveCount(0);
});
