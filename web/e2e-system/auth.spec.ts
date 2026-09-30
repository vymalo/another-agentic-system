import { expect, test } from "@playwright/test";
import { WEB } from "./env";
import {
  ALICE,
  BOB,
  badge,
  errorLine,
  framesOf,
  resetDb,
  startThread,
  threadCount,
  threadId,
} from "./helpers";

test.beforeEach(resetDb);

test("the proxy identity reaches the orchestrator", async ({ page }) => {
  await page.goto("/");
  // /api/agents answered 200 through the app's rewrite, with the identity header
  const agent = page.getByLabel("Agent");
  await expect(agent).toBeVisible();
  await expect(agent.locator("option")).toHaveText(["Coder", "Plain", "Gated"]);

  await page.getByLabel("Agent").selectOption({ label: "Plain" });
  await page.getByLabel("Message").fill("echo hello");
  await page.getByRole("button", { name: "Send" }).click();
  // the connect stream carries the header too: the events arrive and the badge follows
  await expect(badge(page)).toHaveText("Done");

  const frames = await framesOf(page.request, threadId(page));
  const user = frames.find((f) => f.event.type === "TEXT_MESSAGE_START");
  expect(user?.event.metadata).toEqual({ "vymalo.actor": { type: "user", name: ALICE } });
});

test.describe("without the proxy header", () => {
  test.use({ extraHTTPHeaders: {} });

  test("without the proxy header the UI shows the 401 problem, not a crash", async ({ page }) => {
    await page.goto("/");
    const alert = errorLine(page).filter({ hasText: "Could not load agents" });
    await expect(errorLine(page).filter({ hasText: "Could not load threads" })).toBeVisible();
    await expect(alert).toContainText("Could not load agents: missing X-Auth-Request-Email");
    await expect(alert.getByRole("button", { name: "Retry" })).toBeVisible();
    await expect(page.getByLabel("Message")).toBeVisible();
    expect(await threadCount()).toBe(0);

    // the app itself is still fine: nothing threw, Retry only asks again
    await alert.getByRole("button", { name: "Retry" }).click();
    await expect(alert).toContainText("Could not load agents:");
  });
});

test("bob does not see alice's threads", async ({ page, browser }) => {
  await startThread(page, "echo alice only", "Plain");
  await expect(badge(page)).toHaveText("Done");
  const id = threadId(page);

  const bob = await browser.newContext({
    baseURL: WEB,
    extraHTTPHeaders: { "X-Auth-Request-Email": BOB },
  });
  try {
    const bobPage = await bob.newPage();
    await bobPage.goto("/");
    await expect(bobPage.getByText("No threads yet.")).toBeVisible();
    await expect(bobPage.getByText("echo alice only")).toHaveCount(0);

    await bobPage.goto(`/threads/${id}`);
    await expect(bobPage.getByText("Thread not found.")).toBeVisible();
    expect((await bob.request.get(`/api/threads/${id}`)).status()).toBe(404);
    expect((await bob.request.get(`/agui/threads/${id}/connect`)).status()).toBe(404);
    // nor can bob run into it with the same id: existence is not leaked, the id is his to mint
    const run = await bob.request.post("/agui/agents/plain", {
      headers: { Accept: "text/event-stream" },
      data: {
        threadId: id,
        runId: "run-x",
        messages: [{ id: "m-x", role: "user", content: "echo hijack" }],
      },
    });
    expect(run.status()).toBe(404);
  } finally {
    await bob.close();
  }
});
