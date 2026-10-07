import { expect, type Locator, type Page, test } from "@playwright/test";
import { badge, callsFor, conversation, resetDb, startThread, threadId } from "./helpers";

/*
 * Editing a message into a branch (ADR 0029) against the real orchestrator and the fake A2A
 * agents: the edit is a new thread that copies the log before the message and holds the new words,
 * its agent is told the conversation up to there (the history of the fork) and then the new
 * message, in a context of its own, and the two versions are siblings of one message in
 * `GET /api/threads/{id}/branches`. The thread list leaves the edit out unless it is asked for.
 */

test.beforeEach(resetDb);

const PREAMBLE = "[This chat continues an earlier conversation.";

const userMessages = (page: Page) => conversation(page).locator('[data-slot="user-message"]');
const message = (page: Page, text: string) => userMessages(page).filter({ hasText: text });
const picker = (bubble: Locator) => bubble.locator('[data-slot="branch-picker"]');

type Branches = {
  root: string;
  points: { seq: number; index: number; siblings: { threadId: string; seq: number }[] }[];
};
type ListedThread = { id: string; forkedFrom?: { kind: string } };

test("an edit is a new thread whose agent is told the conversation up to the message; the versions are siblings", async ({
  page,
}) => {
  // the first message of a thread names no context and the agent assigns one (ADR 0055), so the calls
  // of this test are found by the words of its own
  const tag = crypto.randomUUID();
  await startThread(page, `echo first ${tag}`, "Plain");
  await expect(badge(page)).toHaveText("Done");
  const parent = threadId(page);
  await page.getByLabel("Message").fill("echo second");
  await page.getByRole("button", { name: "Send" }).click();
  await expect(conversation(page).getByText("echo: echo second")).toBeVisible();
  await expect(badge(page)).toHaveText("Done");

  // the second message, said again: the editor takes its place and Ctrl+Enter sends
  const second = message(page, "echo second");
  await second.hover();
  await second.getByRole("button", { name: "Edit what you said" }).click();
  const editor = page.getByRole("textbox", { name: "What you said" });
  await expect(editor).toBeFocused();
  await editor.fill("recall instead");
  await editor.press("Control+Enter");

  // the new thread's agent recalls what came before the edited message, and only that
  await expect(page).toHaveURL(/\/threads\/[0-9a-f-]{36}#m-\d+$/);
  const edit = threadId(page);
  expect(edit).not.toBe(parent);
  await expect(conversation(page).getByText("recalled: person: echo first")).toBeVisible();
  await expect(badge(page)).toHaveText("Done");
  await expect(userMessages(page)).toHaveText([/^echo first/, /^recall instead/]);
  await expect(conversation(page).getByText("echo second")).toHaveCount(0);

  // the agents' request journals outlive `resetDb`: only the call of this edit, which holds the tag, counts
  const told = (await callsFor(page.request, "plain", PREAMBLE)).filter((c) =>
    c.text.includes(tag),
  );
  expect(told).toHaveLength(1);
  expect(told[0]?.requestedContext).toBeNull();
  expect(told[0]?.text).toContain(`person: echo first ${tag}`);
  expect(told[0]?.text).not.toContain("echo second");
  expect(told[0]?.text.endsWith("recall instead")).toBe(true);

  // the server says the two threads are the versions of one message, the original first
  const branches = (await (
    await page.request.get(`/api/threads/${edit}/branches`)
  ).json()) as Branches;
  expect(branches.root).toBe(parent);
  expect(branches.points).toHaveLength(1);
  expect(branches.points[0]?.index).toBe(1);
  expect(branches.points[0]?.siblings.map((s) => s.threadId)).toEqual([parent, edit]);
  const original = (await (
    await page.request.get(`/api/threads/${parent}/branches`)
  ).json()) as Branches;
  expect(original.points[0]?.index).toBe(0);

  // the page shows it: 2/2 here, and ‹ goes to 1/2 and the original answer
  const edited = message(page, "recall instead");
  await expect(picker(edited)).toContainText("2/2");
  await expect(edited).toBeFocused();
  await picker(edited).getByRole("button", { name: "Previous version" }).click();
  await expect(page).toHaveURL(new RegExp(`/threads/${parent}#m-\\d+$`));
  await expect(conversation(page).getByText("echo: echo second")).toBeVisible();
  await expect(conversation(page).getByText("recall instead")).toHaveCount(0);
  await expect(picker(message(page, "echo second"))).toContainText("1/2");

  // the thread list shows the conversation once; the edit is there when the list is asked for edits
  const listed = (await (await page.request.get("/api/threads")).json()) as ListedThread[];
  expect(listed.map((t) => t.id)).toContain(parent);
  expect(listed.map((t) => t.id)).not.toContain(edit);
  const all = (await (
    await page.request.get("/api/threads?branches=include")
  ).json()) as ListedThread[];
  expect(all.find((t) => t.id === edit)?.forkedFrom?.kind).toBe("edit");
});
