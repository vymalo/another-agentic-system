import { expect, test } from "@playwright/test";
import {
  agentMenu,
  agentMenuItem,
  agentPicker,
  chooseAgent,
  chooseRelease,
  closeAgentMenu,
  openAgentMenu,
  RELEASE_CHANNELS_URI,
} from "./helpers";

test("the release group appears in the menu only for an agent with releases", async ({ page }) => {
  await page.goto("/");
  await chooseAgent(page, "Reviewer");
  await expect(agentPicker(page)).toHaveText(/Reviewer/);
  const menu = await openAgentMenu(page);
  await expect(menu.getByRole("group", { name: "Release" })).toHaveCount(0);
  await closeAgentMenu(page);

  await chooseAgent(page, "Coder");
  // the trigger names the release that will be used, so it is not a surprise
  await expect(agentPicker(page)).toHaveText(/Coder\s*·\s*production/);
  await openAgentMenu(page);
  const group = agentMenu(page).getByRole("group", { name: "Release" });
  await expect(group).toBeVisible();
  await expect(agentMenuItem(page, "production")).toBeChecked();
  await expect(agentMenuItem(page, "production")).toHaveAccessibleName("production — coder-r47");
  await expect(agentMenuItem(page, "staging")).not.toBeChecked();
  // the revisions are listed after the channels: 3 of them
  await expect(group.getByRole("menuitemradio", { name: /^coder-r\d+$/ })).toHaveCount(3);
});

test("the selected release is sent with the new thread", async ({ page }) => {
  await page.goto("/");
  await chooseRelease(page, "staging");
  await page.getByLabel("Message").fill("Use staging");
  const request = page.waitForRequest(
    (r) => r.method() === "POST" && new URL(r.url()).pathname === "/agui/agents/coder",
  );
  await page.getByRole("button", { name: "Send" }).click();
  const body = (await request).postDataJSON();
  // the release travels in forwardedProps under the extension URI (ADR 0008); the text is the message
  // (and the UI catalog, which every new thread is told about: e2e/catalog.spec.ts)
  expect(body.forwardedProps).toMatchObject({ [RELEASE_CHANNELS_URI]: { release: "staging" } });
  expect(Object.keys(body.forwardedProps).sort()).toEqual(
    [RELEASE_CHANNELS_URI, "vymalo.uiCatalog"].sort(),
  );
  expect(body.messages).toMatchObject([{ role: "user", content: "Use staging" }]);
  await expect(page).toHaveURL(/\/threads\//);
  // the agent picker of the top bar names the release, the turn names its revision
  await expect(page.getByText("coder · staging").first()).toBeVisible();
  await expect(page.getByText("coder · coder-r51").first()).toBeVisible();
});

test("an agent without releases sends no release", async ({ page }) => {
  await page.goto("/");
  await chooseAgent(page, "Reviewer");
  await page.getByLabel("Message").fill("Review please");
  const request = page.waitForRequest(
    (r) => r.method() === "POST" && new URL(r.url()).pathname === "/agui/agents/reviewer",
  );
  await page.getByRole("button", { name: "Send" }).click();
  // no release; the UI catalog is the only thing a new thread's first run carries
  expect(Object.keys((await request).postDataJSON().forwardedProps)).toEqual(["vymalo.uiCatalog"]);
});
