import { expect, test } from "@playwright/test";
import {
  agentMenu,
  agentMenuItem,
  agentPicker,
  badge,
  callsFor,
  chooseAgent,
  chooseRelease,
  closeAgentMenu,
  openAgentMenu,
  resetDb,
  startThread,
} from "./helpers";

test.beforeEach(resetDb);

test("coder offers releases, plain does not; staging is echoed as coder-r51", async ({ page }) => {
  await page.goto("/");
  await chooseAgent(page, "Plain");
  await openAgentMenu(page);
  await expect(agentMenu(page).getByRole("group", { name: "Release" })).toHaveCount(0);
  await closeAgentMenu(page);

  await chooseAgent(page, "Adam");
  await expect(agentPicker(page)).toHaveText(/Adam\s*·\s*production/);
  await openAgentMenu(page);
  const release = agentMenu(page).getByRole("group", { name: "Release" });
  await expect(agentMenuItem(page, "production")).toBeChecked();
  await expect(agentMenuItem(page, "production")).toHaveAccessibleName("production — coder-r47");
  await expect(release.getByRole("menuitemradio", { name: /^coder-r\d+$/ })).toHaveCount(3);
  await closeAgentMenu(page);

  await chooseRelease(page, "staging");
  await page.getByLabel("Message").fill("echo use staging");
  await page.getByRole("button", { name: "Send" }).click();
  await expect(page).toHaveURL(/\/threads\//);
  // the agent picker of the top bar names the release
  await expect(page.getByText("adam · staging").first()).toBeVisible();
  await expect(badge(page)).toHaveText("Done");
  await expect(page.getByText("adam · coder-r51").first()).toBeVisible();

  // the extension was activated on the wire, with the selected release
  const [call] = await callsFor(page.request, "adam", "echo use staging");
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
