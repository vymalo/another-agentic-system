import { expect, test } from "@playwright/test";
import { badge, callsFor, resetDb, startThread, threadId } from "./helpers";

test.beforeEach(resetDb);

test("a finished thread takes a follow-up as its next job, in the same A2A context", async ({
  page,
}) => {
  await startThread(page, "echo done", "Plain");
  await expect(badge(page)).toHaveText("Done");
  await expect(page.getByLabel("Message")).toBeEnabled();

  await page.getByLabel("Message").fill("echo one more thing");
  await page.getByRole("button", { name: "Send" }).click();
  await expect(
    page
      .getByRole("log", { name: "Conversation" })
      .getByText("echo one more thing", { exact: true }),
  ).toBeVisible();
  await expect(badge(page)).toHaveText("Done");
  await expect(
    page.getByRole("log", { name: "Conversation" }).getByText("echo: echo one more thing"),
  ).toHaveCount(1);

  // a new A2A task of the same context, not a continuation of the finished one
  const [first] = await callsFor(page.request, "plain", "echo done");
  const [second] = await callsFor(page.request, "plain", "echo one more thing");
  expect(second?.contextId).toBe(first?.contextId);
  // the first message names no context; the agent assigns one, and the second goes on in it (ADR 0055)
  expect(first?.requestedContext).toBeNull();
  expect(second?.requestedContext).toBe(first?.contextId);
  expect(second?.taskId).not.toBe(first?.taskId);

  // the thread is the same, and its log says where the second job began
  const events = await (await page.request.get(`/api/threads/${threadId(page)}/export`)).json();
  expect(events.job.number).toBe(2);
  expect(events.events.filter((e: { kind: string }) => e.kind === "job_started")).toHaveLength(1);
});
