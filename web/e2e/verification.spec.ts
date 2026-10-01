import { expect, test } from "@playwright/test";
import { badge, conversation, expectNoHorizontalScroll, startThread } from "./helpers";

/*
 * The verification gate (ADR 0018), against the mock, which plays the goldens verify-green,
 * verify-red and the two verify-verifier-* of docs/api/examples. `Reviewer` is the agent without
 * releases.
 */

const checks = (page: import("@playwright/test").Page) =>
  conversation(page).getByRole("listitem", { name: /^Check: / });

test("red once: verifying, sent back, verifying again, done; the cards follow", async ({
  page,
}) => {
  await startThread(page, "verify-red-once fix the login", "Reviewer");

  // the states go by at the mock's pace; the pill's steps are the dom tests'
  const failed = conversation(page).getByRole("listitem", {
    name: "Check: Agent checks, attempt 1, failed",
  });
  await expect(failed).toBeVisible();
  // the findings are folded under the check: open them
  await failed.getByText("Findings (1)").click();
  await expect(failed.getByText("tests::login fails: expected 200, got 500")).toBeVisible();
  await expect(conversation(page).getByText("Checks failed — trying again (2/3)")).toBeVisible();

  await expect(badge(page)).toHaveText("Done");
  await expect(checks(page)).toHaveCount(2);
  await expect(
    conversation(page).getByRole("listitem", { name: "Check: Agent checks, attempt 2, passed" }),
  ).toBeVisible();
  await expectNoHorizontalScroll(page);
});

test("red every time: 'Checks failed after 3 attempts', not an ordinary failure", async ({
  page,
}) => {
  test.setTimeout(60_000);
  await startThread(page, "verify-red fix the login", "Reviewer");

  await expect(badge(page)).toHaveText("Failed", { timeout: 30_000 });
  await expect(page.getByText("Checks failed after 3 attempts")).toBeVisible();
  await expect(page.getByText("This thread is failed.")).toHaveCount(0);
  await expect(checks(page)).toHaveCount(3);
  // not locked: the box stays open and says how to go on
  await expect(page.getByLabel("Message")).toBeEnabled();
  await expect(page.getByLabel("Message")).toHaveAttribute(
    "placeholder",
    "Tell the agent how to go on…",
  );
  await expectNoHorizontalScroll(page);
});

test("waiting for CI: a pending card while the thread is verifying; Cancel ends it; a reload shows the same", async ({
  page,
}) => {
  await startThread(page, "verify-wait ship it", "Reviewer");

  await expect(badge(page)).toHaveText("Checking the work…");
  await expect(
    conversation(page).getByRole("listitem", { name: "Check: CI, attempt 1, pending" }),
  ).toBeVisible();
  await page.reload();
  await expect(badge(page)).toHaveText("Checking the work…");
  await expect(checks(page)).toHaveCount(2);

  await page.getByRole("button", { name: "Stop" }).click();
  await expect(badge(page)).toHaveText("Stopped");
  await expect(checks(page)).toHaveCount(2);
});

test("a verifier agent: its findings send the agent back, its pass finishes the thread; a reload shows the same", async ({
  page,
}) => {
  test.setTimeout(60_000);
  await startThread(page, "verify-reviewed fix the login", "Reviewer");

  const failed = conversation(page).getByRole("listitem", {
    name: "Check: Verifier, attempt 1, failed",
  });
  await expect(failed).toBeVisible();
  await failed.getByText("Findings (1)").click();
  await expect(failed.getByText("src/login.rs: the empty password is accepted")).toBeVisible();
  await expect(
    conversation(page).getByText("The review found issues — trying again (2/3)"),
  ).toBeVisible();

  await expect(badge(page)).toHaveText("Done", { timeout: 30_000 });
  await expect(checks(page)).toHaveCount(2);
  await expect(
    conversation(page).getByRole("listitem", { name: "Check: Verifier, attempt 2, passed" }),
  ).toBeVisible();
  await expectNoHorizontalScroll(page);
  await page.reload();
  await expect(badge(page)).toHaveText("Done");
  await expect(checks(page)).toHaveCount(2);
});

test("waiting for the verifier: a pending card; Cancel ends it", async ({ page }) => {
  await startThread(page, "verify-reviewed-wait ship it", "Reviewer");

  await expect(badge(page)).toHaveText("Checking the work…");
  await expect(
    conversation(page).getByRole("listitem", { name: "Check: Verifier, attempt 1, pending" }),
  ).toBeVisible();
  await page.reload();
  await expect(badge(page)).toHaveText("Checking the work…");
  await expect(checks(page)).toHaveCount(1);

  await page.getByRole("button", { name: "Stop" }).click();
  await expect(badge(page)).toHaveText("Stopped");
  await expect(checks(page)).toHaveCount(1);
  await expectNoHorizontalScroll(page);
});

test("the header never shows an attempt counter: attempts are in the turn", async ({ page }) => {
  await startThread(page, "echo hello", "Reviewer");
  await expect(badge(page)).toHaveText("Done");
  await expect(page.locator("[data-slot='attempt-counter']")).toHaveCount(0);
});
