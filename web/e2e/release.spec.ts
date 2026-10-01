import { expect, test } from "@playwright/test";
import { RELEASE_CHANNELS_URI, revisionOptions, selectedOption } from "./helpers";

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
  // the agent's pill (top bar and composer) names the release, the turn names its revision
  await expect(page.getByText("coder · staging").first()).toBeVisible();
  await expect(page.getByText("coder · coder-r51").first()).toBeVisible();
});

test("an agent without releases sends no release", async ({ page }) => {
  await page.goto("/");
  await page.getByLabel("Agent").selectOption({ label: "Reviewer" });
  await page.getByLabel("Message").fill("Review please");
  const request = page.waitForRequest(
    (r) => r.method() === "POST" && new URL(r.url()).pathname === "/agui/agents/reviewer",
  );
  await page.getByRole("button", { name: "Send" }).click();
  // no release; the UI catalog is the only thing a new thread's first run carries
  expect(Object.keys((await request).postDataJSON().forwardedProps)).toEqual(["vymalo.uiCatalog"]);
});
