import { expect, test } from "@playwright/test";
import {
  actorLabel,
  agentMessage,
  badge,
  ECHO,
  framesOf,
  PR_URL,
  resetDb,
  seqs,
  shape,
  startThread,
  threadId,
} from "./helpers";

test.beforeEach(resetDb);

test("echo: user message, Working, PR card, Completed, Done", async ({ page }) => {
  await startThread(page, "echo hello", "Plain");

  const log = page.getByRole("log", { name: "Conversation" });
  await expect(log.getByText("echo hello", { exact: true })).toBeVisible();
  await expect(badge(page)).toHaveText("Done");
  await expect(log.getByText("Working")).toBeVisible();
  await expect(log.getByText("Completed")).toBeVisible();

  const pr = log.getByRole("link", { name: "Pull request acme/demo#1" });
  await expect(pr).toHaveAttribute("href", PR_URL);
  await expect(log.getByText("echo: echo hello")).toHaveCount(1);
  await expect(page.getByLabel("Message")).toBeDisabled();

  // the log behind it: five events, once each (the resume points of the connect stream)
  const frames = await framesOf(page.request, threadId(page));
  expect(shape(frames)).toEqual(ECHO);
  expect(seqs(frames)).toEqual([1, 2, 3, 4, 5]);
});

test("agent text renders once, with the actor", async ({ page }) => {
  await startThread(page, "talk please", "Plain");
  const log = page.getByRole("log", { name: "Conversation" });
  await expect(badge(page)).toHaveText("Done");
  await expect(log.getByText("Working: Reading the repository")).toBeVisible();
  const bubble = agentMessage(page, "Plan: add a test");
  await expect(bubble).toHaveCount(1);
  await expect(actorLabel(bubble)).toHaveText("plain");
  await expect(log.getByText("Plan: add a test")).toHaveCount(1);

  // the same with a release-aware agent: the revision is part of the actor label
  await startThread(page, "talk please", "Coder");
  await expect(badge(page)).toHaveText("Done");
  await expect(actorLabel(agentMessage(page, "Plan: add a test"))).toHaveText("coder · coder-r47");
});
