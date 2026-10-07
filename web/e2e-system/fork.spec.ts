import { expect, test } from "@playwright/test";
import {
  agentMenuItem,
  allCalls,
  badge,
  callsFor,
  conversation,
  openAgentMenu,
  resetDb,
  startThread,
  threadId,
} from "./helpers";

/*
 * Forking a chat (ADR 0029) against the real orchestrator and the fake A2A agents: the fork is a
 * new thread with its own A2A context, and the first message to its agent carries the
 * conversation it continues in front of it (`SendRequest.history`, built by the dispatcher from
 * the fork's own log). The fake agent's `recall` script quotes the first line of what it was told.
 */

test.beforeEach(resetDb);

const PREAMBLE = "[This chat continues an earlier conversation.";

const forkButton = (page: import("@playwright/test").Page) =>
  conversation(page).getByRole("button", { name: "Fork from here" });
const divider = (page: import("@playwright/test").Page) =>
  conversation(page).locator('[data-slot="fork-divider"]');

test("a fork's agent is told the conversation, in a context of its own", async ({ page }) => {
  // The first message of a thread names no context and the agent assigns one (ADR 0055), so a thread's
  // calls are found by words of their own: the fake agents' journals outlive resetDb.
  const tag = crypto.randomUUID();
  await startThread(page, `echo first ${tag}`, "Plain");
  await expect(badge(page)).toHaveText("Done");
  const parent = threadId(page);
  const [first] = await callsFor(page.request, "plain", `echo first ${tag}`);
  expect(first).toBeDefined();
  expect(first?.requestedContext).toBeNull();

  await forkButton(page).click();
  await expect(divider(page)).toContainText("Forked from echo first");
  const fork = threadId(page);
  expect(fork).not.toBe(parent);
  await expect(badge(page)).toHaveText("Done");

  await page.getByLabel("Message").fill("recall and go on");
  await page.getByRole("button", { name: "Send" }).click();
  await expect(conversation(page).getByText("recalled: person: echo first")).toBeVisible();
  await expect(badge(page)).toHaveText("Done");

  // the agent got one message that starts with the conversation and ends with what was said now
  // (the fake agents' journals outlive resetDb: only this fork's, which holds the tag, counts)
  const told = (await callsFor(page.request, "plain", PREAMBLE)).filter((c) =>
    c.text.includes(tag),
  );
  expect(told).toHaveLength(1);
  expect(told[0]?.text).toContain(`person: echo first ${tag}`);
  expect(told[0]?.text.endsWith("recall and go on")).toBe(true);
  // a context of its own: it names none (the agent assigns it), it is not the parent's, and no task
  // of the parent is continued
  expect(told[0]?.requestedContext).toBeNull();
  expect(told[0]?.contextId).not.toBe(first?.contextId);
  expect(told[0]?.taskId).not.toBe(first?.taskId);

  // the parent was not sent anything
  const parentCalls = (await allCalls(page.request, "plain")).filter(
    (c) => c.contextId === first?.contextId && c.kind === "execute",
  );
  expect(parentCalls).toHaveLength(1);
  expect(parent).not.toBe(fork);
});

test("another agent continues the conversation: its first message carries it", async ({ page }) => {
  const tag = crypto.randomUUID();
  await startThread(page, `echo first ${tag}`);
  await expect(badge(page)).toHaveText("Done");
  const parent = threadId(page);

  await openAgentMenu(page);
  await agentMenuItem(page, "Plain").click();
  await page
    .getByRole("alertdialog", { name: "Continue with Plain in a new chat?" })
    .getByRole("button", { name: "Continue in a new chat" })
    .click();
  await expect(divider(page)).toContainText("continued with plain");
  const fork = threadId(page);
  expect(fork).not.toBe(parent);
  expect((await (await page.request.get(`/api/threads/${fork}`)).json()).target.agentId).toBe(
    "plain",
  );

  await page.getByLabel("Message").fill("recall please");
  await page.getByRole("button", { name: "Send" }).click();
  await expect(conversation(page).getByText("recalled: person: echo first")).toBeVisible();
  // the fake agents' journals outlive resetDb: the earlier test's fork also told `plain` its
  // conversation, so only this fork's, which holds the tag, counts
  const told = (await callsFor(page.request, "plain", PREAMBLE)).filter((c) =>
    c.text.includes(tag),
  );
  expect(told).toHaveLength(1);
  expect(told[0]?.text).toContain(`person: echo first ${tag}`);
});
