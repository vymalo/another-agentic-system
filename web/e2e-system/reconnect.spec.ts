import { expect, test } from "@playwright/test";
import { proxyOrigin, WEB } from "./env";
import { FlakyProxy } from "./flaky-proxy";
import {
  activityTab,
  badge,
  releaseGate,
  resetDb,
  showActivity,
  startThread,
  waitForExecution,
} from "./helpers";

const PROXY_PORT = 3101;

// The page is served through a forwarder that can cut the network under the open event stream.
test.use({ baseURL: proxyOrigin(PROXY_PORT) });

const proxy = new FlakyProxy({ host: "127.0.0.1", port: Number(new URL(WEB).port) });
test.beforeAll(() => proxy.start(PROXY_PORT));
test.afterAll(() => proxy.stop());
test.beforeEach(resetDb);

test("a dropped stream resumes without duplicates", async ({ page }) => {
  await startThread(page, "gate reconnect", "Plain");
  await waitForExecution(page.request, "plain", "gate reconnect");
  const log = page.getByRole("log", { name: "Conversation" });
  await expect(badge(page)).toHaveText("Working…");

  proxy.block();
  await expect(page.getByText("Reconnecting…")).toBeVisible();

  // the agent finishes while the browser is cut off; the control endpoint is not behind the proxy
  await releaseGate(page.request, "plain");
  proxy.unblock();

  await expect(badge(page)).toHaveText("Done");
  await expect(page.getByText("Reconnecting…")).toHaveCount(0);
  await expect(log.getByText("gate reconnect", { exact: true })).toHaveCount(1);
  await showActivity(page);
  await expect(activityTab(page).getByText("Started working", { exact: true })).toHaveCount(1);
  await expect(activityTab(page).getByText("Opened pull request #1")).toHaveCount(1);
  await expect(log.getByRole("link", { name: /^View pull request / })).toHaveCount(1);
  await expect(log.getByText("echo: gate reconnect")).toHaveCount(1);
});
