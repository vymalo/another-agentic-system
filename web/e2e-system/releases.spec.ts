import { expect, test } from "@playwright/test";
import { badge, callsFor, resetDb, revisionOptions, selectedOption, startThread } from "./helpers";

test.beforeEach(resetDb);

test("coder offers releases, plain does not; staging is echoed as coder-r51", async ({ page }) => {
  await page.goto("/");
  await page.getByLabel("Agent").selectOption({ label: "Plain" });
  await expect(page.getByLabel("Release")).toHaveCount(0);

  await page.getByLabel("Agent").selectOption({ label: "Coder" });
  const release = page.getByLabel("Release");
  await expect(release).toHaveValue("production");
  await expect(selectedOption(release)).toHaveText("production — coder-r47");
  await expect(revisionOptions(release)).toHaveCount(3);

  await release.selectOption("staging");
  await page.getByLabel("Message").fill("echo use staging");
  await page.getByRole("button", { name: "Send" }).click();
  await expect(page).toHaveURL(/\/threads\//);
  // the agent's pill (top bar and composer) names the release
  await expect(page.getByText("coder · staging").first()).toBeVisible();
  await expect(badge(page)).toHaveText("Done");
  await expect(page.getByText("coder · coder-r51").first()).toBeVisible();

  // the extension was activated on the wire, with the selected release
  const [call] = await callsFor(page.request, "coder", "echo use staging");
  expect(call?.activatesReleaseChannels).toBe(true);
  expect(call?.release).toBe("staging");
});

test("an agent without releases gets no extension activation", async ({ page }) => {
  await startThread(page, "echo no release", "Plain");
  await expect(badge(page)).toHaveText("Done");
  const [call] = await callsFor(page.request, "plain", "echo no release");
  expect(call?.activatesReleaseChannels).toBe(false);
  expect(call?.release).toBeNull();
});
