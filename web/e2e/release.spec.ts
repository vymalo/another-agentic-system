import { expect, test } from "@playwright/test";
import { revisionOptions, selectedOption } from "./helpers";

test("the release dropdown appears only for an agent with releases", async ({ page }) => {
  await page.goto("/");
  await page.getByLabel("Agent").selectOption({ label: "Reviewer" });
  await expect(page.getByLabel("Release")).toHaveCount(0);

  await page.getByLabel("Agent").selectOption({ label: "Coder" });
  const release = page.getByLabel("Release");
  await expect(release).toBeVisible();
  await expect(release).toHaveValue("production");
  await expect(selectedOption(release)).toHaveText("production — coder-r47");
  await expect(revisionOptions(release)).toHaveCount(3);
});

test("the selected release is sent with the new thread", async ({ page }) => {
  await page.goto("/");
  await page.getByLabel("Release").selectOption("staging");
  await page.getByLabel("Message").fill("Use staging");
  const request = page.waitForRequest(
    (r) => r.method() === "POST" && new URL(r.url()).pathname === "/api/threads",
  );
  await page.getByRole("button", { name: "Send" }).click();
  expect((await request).postDataJSON()).toEqual({
    target: { agentId: "coder", release: "staging" },
    text: "Use staging",
  });
  await expect(page).toHaveURL(/\/threads\//);
  await expect(page.getByText("coder · staging")).toBeVisible();
  await expect(page.getByText("coder · coder-r51").first()).toBeVisible();
});

test("an agent without releases sends no release", async ({ page }) => {
  await page.goto("/");
  await page.getByLabel("Agent").selectOption({ label: "Reviewer" });
  await page.getByLabel("Message").fill("Review please");
  const request = page.waitForRequest(
    (r) => r.method() === "POST" && new URL(r.url()).pathname === "/api/threads",
  );
  await page.getByRole("button", { name: "Send" }).click();
  expect((await request).postDataJSON()).toEqual({
    target: { agentId: "reviewer" },
    text: "Review please",
  });
});
