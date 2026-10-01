import { readFile } from "node:fs/promises";
import { expect, test } from "@playwright/test";
import {
  actorLabel,
  agentMessage,
  badge,
  ECHO,
  exportMenuItem,
  framesOf,
  PR_URL,
  resetDb,
  seqs,
  shape,
  startThread,
  threadId,
} from "./helpers";

test.beforeEach(resetDb);

test("echo: user message, its steps, the PR card, Done", async ({ page }) => {
  await startThread(page, "echo hello", "Plain");

  const log = page.getByRole("log", { name: "Conversation" });
  await expect(log.getByText("echo hello", { exact: true })).toBeVisible();
  await expect(badge(page)).toHaveText("Done");
  await expect(log.getByText("Started working")).toBeVisible();
  await expect(log.getByText("Opened pull request #1")).toBeVisible();

  const pr = log.getByRole("link", { name: "Pull request acme/demo#1" });
  await expect(pr).toHaveAttribute("href", PR_URL);
  await expect(log.getByText("echo: echo hello")).toHaveCount(1);
  // a finished thread is not locked (ADR 0020)
  await expect(page.getByLabel("Message")).toBeEnabled();

  // the log behind it: five events, once each (the resume points of the connect stream), after
  // the web's UI catalog, which the run that created the thread carried: it is the log's first
  // event (seq 1) and has no frame (ADR 0023)
  const frames = await framesOf(page.request, threadId(page));
  expect(shape(frames)).toEqual(ECHO);
  expect(seqs(frames)).toEqual([2, 3, 4, 5, 6]);
});

test("agent text renders once, with the actor", async ({ page }) => {
  await startThread(page, "talk please", "Plain");
  const log = page.getByRole("log", { name: "Conversation" });
  await expect(badge(page)).toHaveText("Done");
  await expect(log.getByText("Reading the repository", { exact: true })).toBeVisible();
  const bubble = agentMessage(page, "Plan: add a test");
  await expect(bubble).toHaveCount(1);
  await expect(actorLabel(bubble)).toHaveText("plain");
  await expect(log.getByText("Plan: add a test")).toHaveCount(1);

  // the same with a release-aware agent: the revision is part of the actor label
  await startThread(page, "talk please", "Coder");
  await expect(badge(page)).toHaveText("Done");
  await expect(actorLabel(agentMessage(page, "Plan: add a test"))).toHaveText("coder · coder-r47");
});

test("Export JSON: the real orchestrator's whole log in one downloaded file", async ({ page }) => {
  await startThread(page, "echo hello", "Plain");
  await expect(badge(page)).toHaveText("Done");
  const id = threadId(page);

  const [download] = await Promise.all([
    page.waitForEvent("download"),
    (await exportMenuItem(page)).click(),
  ]);
  expect(download.suggestedFilename()).toBe(`thread-${id}.json`);
  const doc = JSON.parse(await readFile(await download.path(), "utf8"));
  expect(doc.format).toBe("another-agentic-system/thread-export");
  expect(doc.version).toBe(1);
  expect(doc.thread.id).toBe(id);
  expect(doc.binding.agentId).toBe("plain");
  expect(doc.eventsTruncated).toBe(false);
  const log: { seq: number; kind: string }[] = doc.events;
  expect(log.map((e) => e.kind)).toEqual([
    "ui_catalog", // the web's catalog, sent with the run that created the thread (ADR 0023)
    "user_message",
    "agent_status",
    "artifact",
    "agent_status",
    "thread_state",
  ]);
  expect(log.map((e) => e.seq)).toEqual([1, 2, 3, 4, 5, 6]);
  // the same log the connect stream replays, but for the catalog, which has no frame
  expect(seqs(await framesOf(page.request, id))).toEqual(
    log.filter((e) => e.kind !== "ui_catalog").map((e) => e.seq),
  );
});
