import { expect, test } from "@playwright/test";
import { badge, startThread } from "./helpers";

test("create a thread and watch it finish", async ({ page }) => {
  await startThread(page, "Implement the thing");

  const log = page.getByRole("log", { name: "Conversation" });
  await expect(log.getByText("Implement the thing", { exact: true })).toBeVisible();
  await expect(log.getByText("Working")).toBeVisible();

  const pr = page.getByRole("link", { name: "Pull request vymalo/example#1" });
  await expect(pr).toBeVisible();
  await expect(pr).toHaveAttribute("href", "https://github.com/vymalo/example/pull/1");

  // the partial agent message was replaced by its final version, and renders once
  const final = log.getByText("make the smallest change that fixes it", { exact: false });
  await expect(final).toHaveCount(1);
  await expect(log.getByText("I'll start with the failing test")).toHaveCount(1);

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
