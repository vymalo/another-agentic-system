import { expect, test } from "@playwright/test";
import { badge, startThread } from "./helpers";

test("create a thread and watch it finish", async ({ page }) => {
  await startThread(page, "Implement the thing");

  const log = page.getByRole("log", { name: "Conversation" });
  await expect(log.getByText("Implement the thing", { exact: true })).toBeVisible();
  await expect(log.getByText("Working")).toBeVisible();

  const pr = page.getByRole("link", { name: "Pull request acme/demo#1" });
  await expect(pr).toBeVisible();
  await expect(pr).toHaveAttribute("href", "https://github.com/acme/demo/pull/1");
  await expect(log.getByText("echo: Implement the thing")).toHaveCount(1);

  await expect(badge(page)).toHaveText("Done");
  await expect(page.getByLabel("Message")).toBeDisabled();
  await expect(page.getByText("This thread is done.")).toBeVisible();

  // the thread shows up in the list (on a phone the list is a collapsed disclosure)
  const disclosure = page.locator("summary.sidebar__summary");
  if (await disclosure.isVisible()) await disclosure.click();
  await expect(
    page
      .getByRole("navigation", { name: "Threads" })
      .getByRole("button", { name: "Implement the thing" })
      .first(),
  ).toBeVisible();
});

test("agent text renders once, with a status line and the actor", async ({ page }) => {
  await startThread(page, "talk to me");

  const log = page.getByRole("log", { name: "Conversation" });
  await expect(badge(page)).toHaveText("Done");
  await expect(log.getByText("Working: Reading the repository")).toBeVisible();
  await expect(log.getByText("Plan: add a test")).toHaveCount(1);
  await expect(
    log.locator(".msg--agent", { hasText: "Plan: add a test" }).locator(".actor"),
  ).toHaveText("coder · coder-r47");
});

test("a partial agent message is replaced by its final version and renders once", async ({
  page,
}) => {
  // Not produced by the current orchestrator (it reports every message as final); the UI still
  // has to handle it, and the reducer is only checked with fixtures otherwise.
  await startThread(page, "partial message please");

  const log = page.getByRole("log", { name: "Conversation" });
  await expect(badge(page)).toHaveText("Done");
  await expect(
    log.getByText("make the smallest change that fixes it", { exact: false }),
  ).toHaveCount(1);
  await expect(log.getByText("I'll start with the failing test")).toHaveCount(1);
});
