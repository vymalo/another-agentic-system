import { expect, type Page, test } from "@playwright/test";
import { activityTab, badge, conversation, showActivity, startThread } from "./helpers";

// The mock's test hooks (mock/server.ts), reached on the mock's own origin.
const MOCK = "http://127.0.0.1:4010";

/** The thread the page is on: the hooks of this file are scoped to it, so tests running side by side do not spend each other's. */
const threadOf = (page: Page) => /\/threads\/([0-9a-f-]{36})$/.exec(page.url())?.[1] ?? "";

test("a dropped connection resumes with Last-Event-ID: nothing is missing, nothing twice", async ({
  page,
  request,
}) => {
  const connects: (string | undefined)[] = [];
  page.on("request", (r) => {
    if (new URL(r.url()).pathname.endsWith("/connect")) {
      connects.push(r.headers()["last-event-id"]);
    }
  });
  await startThread(page, "talk to me");
  // the steps are the panel's
  await showActivity(page);
  await expect(
    activityTab(page).getByText("Reading the repository", { exact: true }),
  ).toBeVisible();

  // the network goes away in the middle of the run
  expect((await request.post(`${MOCK}/__mock/drop-streams?thread=${threadOf(page)}`)).ok()).toBe(
    true,
  );

  await expect(badge(page)).toHaveText("Done");
  const log = conversation(page);
  const steps = activityTab(page);
  await expect(log.getByText("Plan: add a test")).toHaveCount(1);
  await expect(steps.getByText("Started working", { exact: true })).toHaveCount(1);
  await expect(steps.getByText("Reading the repository", { exact: true })).toHaveCount(1);
  await expect(steps.getByText("Opened pull request #1")).toHaveCount(1);
  await expect(log.getByRole("link", { name: /^View pull request / })).toHaveCount(1);
  // the first connect had no cursor; the reconnect carried one
  expect(connects[0]).toBeUndefined();
  expect(connects.length).toBeGreaterThanOrEqual(2);
  expect(connects.at(-1)).toMatch(/^\d+$/);
});

test("a connection cut in the middle of a message is repeated from the last resume point", async ({
  page,
  request,
}) => {
  await startThread(page, "talk to me");
  await expect(badge(page)).toHaveText("Done");

  // reload: the replay is cut after 12 frames, in the middle of the group that carries the text
  expect((await request.post(`${MOCK}/__mock/cut-next-connect?frames=12`)).ok()).toBe(true);
  await page.reload();
  const log = conversation(page);
  await expect(log.getByText("Plan: add a test")).toHaveCount(1);
  await showActivity(page);
  await expect(activityTab(page).getByText("Opened pull request #1")).toHaveCount(1);
  await expect(activityTab(page).getByText("Reading the repository", { exact: true })).toHaveCount(
    1,
  );
  await expect(badge(page)).toHaveText("Done");
});

// ADR 0082, C3: "hold a stream idle — it stays up, or comes back within 45 s". Here the socket is held
// open and says nothing at all, which is what a proxy or a dead NAT entry leaves behind: the page must
// notice the *silence*, not a close, and reopen from the cursor it reached. One heartbeat missed is a
// stream that is merely quiet, so nothing may reopen before the second one.
test("a stream that goes silent is reopened from its cursor after two heartbeats, and nothing is lost", async ({
  page,
  request,
}) => {
  // two missed heartbeats (2 × 15 s) and the reopen: the requirement's own 45 s
  test.setTimeout(120_000);
  const connects: (string | undefined)[] = [];
  page.on("request", (r) => {
    if (new URL(r.url()).pathname.endsWith("/connect")) {
      connects.push(r.headers()["last-event-id"]);
    }
  });
  // a run that waits for the test: the stream is open, and says nothing but its keepalives
  await startThread(page, "gate hold");
  await expect(badge(page)).toHaveText("Working…");
  const thread = threadOf(page);
  await expect.poll(() => connects.length, { timeout: 10_000 }).toBe(1);

  // the keepalives stop: the connection stays open, so only the silence says the stream is dead
  expect((await request.post(`${MOCK}/__mock/silence-streams?thread=${thread}`)).ok()).toBe(true);

  // one missed heartbeat: still quiet, and still followed
  await page.waitForTimeout(29_000);
  expect(connects.length).toBe(1);

  // the second: it reopens from the cursor it reached, inside the 45 s the requirement allows
  await expect.poll(() => connects.length, { timeout: 16_000 }).toBe(2);
  expect(connects.at(-1)).toMatch(/^\d+$/);

  // and the run goes on: the reply the reopened stream carries is the one the run wrote
  expect((await request.post(`${MOCK}/__mock/release?thread=${thread}`)).status()).toBe(204);
  await expect(badge(page)).toHaveText("Done", { timeout: 30_000 });
  await expect(conversation(page).getByText("echo: gate hold")).toHaveCount(1);
});
