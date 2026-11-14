import { test as base, expect, type Page } from "@playwright/test";
import { uuidv7 } from "../src/lib/uuid";
import {
  badge,
  conversation,
  expectNoHorizontalScroll,
  MOCK_URL,
  startThread,
  THREAD_URL,
} from "./helpers";

/*
 * Regenerate the last answer (ADR 0029, amended 2026-11-14, web/DESIGN.md "Fork and branch"): under the
 * newest finished turn, a button that says the person's message again with the same words, as an edit.
 * The answer lands as a new version of the message, and the `‹ 2/2 ›` of the branch picker goes between
 * the two. It is not offered on an earlier turn, while the turn is going on, to a person who may only
 * read, on a shared link, or when the turn began with a form answer or a mention (an edit carries
 * neither). The mock server plays the contract of `forkThread` with `replace`; nothing waits for a timer.
 */

const ORIGIN = "http://127.0.0.1:3000";

const test = base.extend<{ asReadOnly: () => Promise<void> }>({
  asReadOnly: async ({ context }, use, info) => {
    const session = `e2e-regen-${info.workerIndex}-${info.testId}`;
    await use(async () => {
      const res = await fetch(`${MOCK_URL}/__mock/config?me=read-only&session=${session}`, {
        method: "POST",
      });
      expect(res.ok).toBe(true);
      await context.addCookies([{ name: "mock-registry", value: session, url: ORIGIN }]);
    });
  },
});

const regenerate = (page: Page) => conversation(page).getByRole("button", { name: "Regenerate" });
const forkButton = (page: Page) =>
  conversation(page).getByRole("button", { name: "Fork from here" });
const userMessages = (page: Page) => conversation(page).locator('[data-slot="user-message"]');
const picker = (page: Page) => userMessages(page).last().locator('[data-slot="branch-picker"]');
const threadIdOf = (url: string) => new URL(url).pathname.split("/").pop() as string;

/** Says `text` in the open thread and waits for the agent to be done with it. */
async function say(page: Page, text: string) {
  await page.getByLabel("Message", { exact: true }).fill(text);
  await page.getByRole("button", { name: "Send" }).click();
  await expect(conversation(page).getByText(`echo: ${text}`)).toBeVisible();
  await expect(badge(page)).toHaveText("Done");
}

test("regenerate makes a second version of the message, and the arrows go back to the first", async ({
  page,
}) => {
  await startThread(page, "echo first");
  await expect(badge(page)).toHaveText("Done");
  await say(page, "echo second");
  await expect(userMessages(page)).toHaveCount(2);
  const original = page.url();
  // the first turn is an earlier one: the button is under the last only
  await expect(regenerate(page)).toHaveCount(1);
  await expect(regenerate(page)).toBeEnabled();
  await expect(regenerate(page)).toHaveAttribute("aria-disabled", "false");

  // the request is an edit with the same words and no access tier
  const bodies: Record<string, unknown>[] = [];
  page.on("request", (r) => {
    if (r.method() === "POST" && r.url().endsWith("/fork")) bodies.push(r.postDataJSON());
  });
  await regenerate(page).click();

  await expect(page).not.toHaveURL(original);
  await expect(page).toHaveURL(/\/threads\/[0-9a-f-]{36}#m-\d+$/);
  await expect(badge(page)).toHaveText("Done");
  await expect(userMessages(page)).toHaveText([/^echo first/, /^echo second/]);
  await expect(conversation(page).getByText("echo: echo second")).toBeVisible();
  expect(bodies).toHaveLength(1);
  expect(bodies[0]).toMatchObject({ text: "echo second", messageId: expect.any(String) });
  expect(typeof bodies[0]?.replace).toBe("number");
  expect(bodies[0]).not.toHaveProperty("tier");
  await expectNoHorizontalScroll(page);

  // the new version is 2/2, and ‹ goes back to the original and its answer
  await expect(picker(page)).toContainText("2/2");
  await expect(picker(page).getByRole("status")).toHaveText("Version 2 of 2");
  await picker(page).getByRole("button", { name: "Previous version" }).click();
  await expect(page).toHaveURL(new RegExp(`${threadIdOf(original)}#m-\\d+$`));
  await expect(picker(page)).toContainText("1/2");
  await expect(conversation(page).getByText("echo: echo second")).toBeVisible();
  // and the original's last turn can be regenerated again: it is its newest turn
  await expect(regenerate(page)).toBeEnabled();
});

test("it is not offered while the turn is going on, and says why", async ({ page }) => {
  await startThread(page, "Refactor the module");
  await expect(badge(page)).toHaveText("Working…");
  await expect(regenerate(page)).toHaveAttribute("aria-disabled", "true");
  await regenerate(page).hover();
  await expect(page.getByRole("tooltip")).toContainText("once the agent has finished this turn");
  // a click does nothing: no request is made
  let requested = false;
  page.on("request", (r) => {
    if (r.url().endsWith("/fork")) requested = true;
  });
  await regenerate(page).click({ force: true });
  expect(requested).toBe(false);
  await expect(page).toHaveURL(THREAD_URL);

  await page.getByRole("button", { name: "Stop" }).click();
  await expect(badge(page)).not.toHaveText("Working…");
  await expect(regenerate(page)).toHaveAttribute("aria-disabled", "false");
});

test("it is not on an earlier turn", async ({ page }) => {
  await startThread(page, "echo first");
  await expect(badge(page)).toHaveText("Done");
  await say(page, "echo second");
  await expect(forkButton(page)).toHaveCount(2);
  await expect(regenerate(page)).toHaveCount(1);
  // it sits in the last turn
  await expect(
    conversation(page).locator('[data-slot="agent-turn"]').last().getByRole("button", {
      name: "Regenerate",
    }),
  ).toHaveCount(1);
});

test("it is not offered to a person who may only read", async ({ page, asReadOnly }) => {
  const id = uuidv7();
  const res = await fetch(`${MOCK_URL}/agui/agents/adam`, {
    method: "POST",
    headers: { "Content-Type": "application/json", Accept: "text/event-stream" },
    body: JSON.stringify({
      threadId: id,
      runId: "run-1",
      messages: [{ id: "m-1", role: "user", content: "echo regen read only" }],
    }),
  });
  expect(res.ok).toBe(true);
  await res.text();
  // the read-only person is viewer@example.com: the thread is theirs, and they may not write in it
  const handed = await fetch(`${MOCK_URL}/__mock/owner?thread=${id}&owner=viewer@example.com`, {
    method: "POST",
  });
  expect(handed.status).toBe(204);
  await asReadOnly();
  await page.goto(`/threads/${id}`);
  await expect(badge(page)).toHaveText("Done");
  await expect(conversation(page).getByText("echo: echo regen read only")).toBeVisible();
  await conversation(page).hover();
  await expect(regenerate(page)).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Fork from here" })).toHaveCount(0);
});

test("it is not offered on a shared link", async ({ page, context }) => {
  // the owner makes a thread and shares it; a reader of another session opens the link
  const sessionOf = async (query: string) => {
    const session = `e2e-regen-share-${process.pid}-${Date.now()}-${query.replace(/\W/g, "")}`;
    const res = await fetch(`${MOCK_URL}/__mock/config?${query}&session=${session}`, {
      method: "POST",
    });
    expect(res.ok).toBe(true);
    return session;
  };
  const owner = `mock-registry=${await sessionOf("sharing=public")}`;
  const id = uuidv7();
  const run = await fetch(`${MOCK_URL}/agui/agents/adam`, {
    method: "POST",
    headers: { "Content-Type": "application/json", Accept: "text/event-stream", cookie: owner },
    body: JSON.stringify({
      threadId: id,
      runId: "run-1",
      messages: [{ id: "m-1", role: "user", content: "echo regen shared" }],
    }),
  });
  expect(run.ok).toBe(true);
  await run.text();
  const shared = await fetch(`${MOCK_URL}/api/threads/${id}/share`, {
    method: "PUT",
    headers: { "Content-Type": "application/json", cookie: owner },
    body: JSON.stringify({ visibility: "internal" }),
  });
  expect(shared.status).toBe(200);
  const token = ((await shared.json()) as { url: string }).url.replace("/s/", "");

  const reader = await sessionOf("me=admin&sharing=public");
  await context.addCookies([{ name: "mock-registry", value: reader, url: ORIGIN }]);
  await page.goto(`/s/${token}`);
  await expect(page.locator('[data-slot="shared-banner"]')).toHaveText(
    "Shared conversation, read only",
  );
  await expect(badge(page)).toHaveText("Done", { timeout: 20_000 });
  await expect(conversation(page).getByText("echo: echo regen shared")).toBeVisible();
  await conversation(page).hover();
  await expect(regenerate(page)).toHaveCount(0);
});

test("it is not offered when the message mentioned an agent: an edit carries no mentions", async ({
  page,
}) => {
  await page.goto("/");
  const box = page.getByLabel("Message", { exact: true });
  await box.focus();
  await page.keyboard.type("echo ask @");
  // the agents may still be loading: an Enter before the list is a send
  await expect(page.getByRole("listbox", { name: "Agents to mention" })).toBeVisible();
  await page.keyboard.press("Enter");
  await page.keyboard.type("to look");
  await page.keyboard.press("Enter");
  await expect(page).toHaveURL(THREAD_URL);
  await expect(badge(page)).toHaveText("Done");
  await expect(conversation(page).locator('[data-slot="mention"]')).toHaveCount(1);
  await conversation(page).hover();
  await expect(forkButton(page)).toBeVisible();
  await expect(regenerate(page)).toHaveCount(0);
});

test("it is not offered on a turn that began with a form answer", async ({ page }) => {
  await startThread(page, "choices now", "Reviewer");
  await expect(badge(page)).toHaveText("Your turn");
  const ui = conversation(page).getByRole("region", { name: "Interface from reviewer" });
  for (const [group, label, role] of [
    ["Which database?", "Postgres", "radio"],
    ["Which login?", "No login", "radio"],
    ["Where does it run?", "Docker Compose", "checkbox"],
  ] as const) {
    const g = ui.getByRole(role === "radio" ? "radiogroup" : "group", {
      name: role === "radio" ? group : new RegExp(group.replace("?", "\\?")),
    });
    await g.getByText(label, { exact: true }).click();
  }
  await page.getByRole("button", { name: "Send answers" }).click();
  await expect(badge(page)).toHaveText("Done");
  await expect(page.locator('[data-slot="answer-bubble"]')).toBeVisible();
  await conversation(page).hover();
  await expect(regenerate(page)).toHaveCount(0);
});

/** The `/fork` requests the page makes, with their bodies. */
function forks(page: Page) {
  const bodies: Record<string, unknown>[] = [];
  page.on("request", (r) => {
    if (r.method() === "POST" && r.url().endsWith("/fork")) bodies.push(r.postDataJSON());
  });
  return bodies;
}

test("regenerate twice: on the new version, and after going back, each is a request of its own", async ({
  page,
}) => {
  await startThread(page, "echo only");
  await expect(badge(page)).toHaveText("Done");
  const first = page.url();
  const bodies = forks(page);

  await regenerate(page).click();
  await expect(page).not.toHaveURL(first);
  await expect(badge(page)).toHaveText("Done");
  await expect(picker(page)).toContainText("2/2");

  // a second regenerate, on the new version
  const second = page.url();
  await regenerate(page).click();
  await expect(page).not.toHaveURL(second);
  await expect(badge(page)).toHaveText("Done");
  await expect(picker(page)).toContainText("3/3");

  // and one after going back to the first version
  await page.goto(first);
  await expect(badge(page)).toHaveText("Done");
  await regenerate(page).click();
  await expect(page).not.toHaveURL(first);
  await expect(badge(page)).toHaveText("Done");
  await expect(picker(page)).toContainText("4/4");

  expect(bodies).toHaveLength(3);
  expect(new Set(bodies.map((b) => b.messageId)).size).toBe(3);
  expect(new Set(bodies.map((b) => b.id)).size).toBe(3);
});

test("a retry of a refused regenerate is the same request", async ({ page }) => {
  await startThread(page, "echo retry");
  await expect(badge(page)).toHaveText("Done");
  const bodies = forks(page);
  let refuse = true;
  await page.route("**/api/threads/*/fork", async (route) => {
    if (!refuse) return route.continue();
    refuse = false;
    return route.fulfill({
      status: 503,
      contentType: "application/problem+json",
      body: JSON.stringify({ title: "Unavailable", status: 503, detail: "try again" }),
    });
  });
  await regenerate(page).click();
  await expect(
    page.getByRole("alert").filter({ hasText: "Could not regenerate the answer" }),
  ).toBeVisible();
  await regenerate(page).click();
  await expect(page).toHaveURL(/#m-\d+$/);
  expect(bodies).toHaveLength(2);
  expect(bodies[1]?.id).toBe(bodies[0]?.id);
  expect(bodies[1]?.messageId).toBe(bodies[0]?.messageId);
});

test("a refused regenerate says so, and what to do instead", async ({ page }) => {
  await startThread(page, "echo refused");
  await expect(badge(page)).toHaveText("Done");
  await page.route("**/api/threads/*/fork", (route) =>
    route.fulfill({
      status: 422,
      contentType: "application/problem+json",
      body: JSON.stringify({
        title: "Unprocessable",
        status: 422,
        detail: "the tier cannot be kept",
      }),
    }),
  );
  const here = page.url();
  await regenerate(page).click();
  const alert = page.getByRole("alert").filter({ hasText: "Could not regenerate" });
  await expect(alert).toContainText(
    "Could not regenerate the answer: the tier cannot be kept. You can edit the message instead.",
  );
  await expect(page).toHaveURL(here);
});

test("regenerate sends no tier, even when the composer has one", async ({ page, context }) => {
  const name = `e2e-regen-tier-${process.pid}-${Date.now()}`;
  await context.addCookies([{ name: "mock-registry", value: name, url: ORIGIN }]);
  const lists = await fetch(`${MOCK_URL}/__mock/tiers?session=${name}`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({
      agent: "adam",
      params: { tiers: ["plan", "edits", "auto", "bypass"], default: "auto" },
    }),
  });
  expect(lists.ok).toBe(true);
  const runs: { forwardedProps: Record<string, unknown> }[] = [];
  page.on("request", (r) => {
    if (r.method() === "POST" && /\/agui\/agents\//.test(r.url())) runs.push(r.postDataJSON());
  });
  await page.goto("/");
  await page.getByRole("button", { name: /^Access tier:/ }).click();
  await page.getByRole("menuitemradio").filter({ hasText: /Edits/ }).click();
  await page.getByLabel("Message", { exact: true }).fill("echo tiered");
  await page.getByRole("button", { name: "Send" }).click();
  await expect(page).toHaveURL(THREAD_URL);
  await expect(badge(page)).toHaveText("Done");
  // the composer has a tier, and the first message went out under it
  expect(runs[0]?.forwardedProps["vymalo.tier"]).toBe("edits");
  const bodies = forks(page);
  await regenerate(page).click();
  await expect(page).toHaveURL(/#m-\d+$/);
  expect(bodies).toHaveLength(1);
  expect(bodies[0]).toMatchObject({ text: "echo tiered" });
  expect(bodies[0]).not.toHaveProperty("tier");
});

test("it is not offered while a question waits for an answer", async ({ page }) => {
  await startThread(page, "choices now", "Reviewer");
  await expect(
    conversation(page).getByRole("region", { name: "Interface from reviewer" }),
  ).toBeVisible();
  await expect(badge(page)).not.toHaveText("Working…");
  await conversation(page).hover();
  await expect(regenerate(page)).toHaveCount(0);
});

test("it is not offered on a turn that answers a message sent while the agent worked", async ({
  page,
}) => {
  await startThread(page, "gate refactor the parser");
  await expect(badge(page)).toHaveText("Working…");
  await page.getByLabel("Message", { exact: true }).fill("echo you were wrong");
  await page.getByRole("button", { name: "Send" }).click();
  await expect(conversation(page).locator('[data-slot="delivery-note"]')).toHaveText(
    "Sent while Adam was working · read at its next step",
  );
  const id = threadIdOf(page.url());
  const released = await fetch(`${MOCK_URL}/__mock/release?thread=${id}`, { method: "POST" });
  expect(released.status).toBe(204);
  await expect(conversation(page).getByText("echo: echo you were wrong")).toBeVisible();
  await expect(badge(page)).toHaveText("Done");
  await conversation(page).hover();
  await expect(regenerate(page)).toHaveCount(0);
});
