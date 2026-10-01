import { expect, test } from "@playwright/test";
import { badge, conversation, startThread } from "./helpers";

// The mock's test hooks (mock/server.ts), reached on the mock's own origin.
const MOCK = "http://127.0.0.1:4010";

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
  await expect(
    conversation(page).getByText("Reading the repository", { exact: true }),
  ).toBeVisible();

  // the network goes away in the middle of the run
  expect((await request.post(`${MOCK}/__mock/drop-streams`)).ok()).toBe(true);

  await expect(badge(page)).toHaveText("Done");
  const log = conversation(page);
  await expect(log.getByText("Plan: add a test")).toHaveCount(1);
  await expect(log.getByText("Started working", { exact: true })).toHaveCount(1);
  await expect(log.getByText("Reading the repository", { exact: true })).toHaveCount(1);
  await expect(log.getByText("Opened pull request #1")).toHaveCount(1);
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
  await expect(log.getByText("Opened pull request #1")).toHaveCount(1);
  await expect(log.getByText("Reading the repository", { exact: true })).toHaveCount(1);
  await expect(badge(page)).toHaveText("Done");
});
