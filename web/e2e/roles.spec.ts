import AxeBuilder from "@axe-core/playwright";
import { test as base, expect, type Page } from "@playwright/test";
import { uuidv7 } from "../src/lib/uuid";
import {
  agentMenu,
  agentPicker,
  animationsDone,
  badge,
  conversation,
  errorLine,
  expectNoHorizontalScroll,
  MOCK_URL,
  openAgentMenu,
  openThreadList,
  threadList,
} from "./helpers";

/*
 * Who the person is and what their roles let them do (ADR 0033), against the mock: `GET /api/me`
 * says it, the page shows and hides by it, and the mock refuses what it hides, as the orchestrator
 * does. The mock keeps who a session is per session, named by a cookie, so a test is its own person
 * without turning the tests beside it into someone else.
 */

const ORIGIN = "http://127.0.0.1:3000";

type Profile = "user" | "admin" | "read-only" | "limited" | "no-access";

const test = base.extend<{ as: (profile: Profile) => Promise<void> }>({
  as: async ({ context }, use, info) => {
    const session = `e2e-${info.workerIndex}-${info.testId}`;
    await use(async (profile) => {
      const res = await fetch(`${MOCK_URL}/__mock/config?me=${profile}&session=${session}`, {
        method: "POST",
      });
      expect(res.ok).toBe(true);
      await context.addCookies([{ name: "mock-registry", value: session, url: ORIGIN }]);
    });
  },
});

/** A thread of the default person (dev@example.com), run to its end; its title is its first words. */
async function threadOfDev(title: string): Promise<string> {
  const id = uuidv7();
  const res = await fetch(`${MOCK_URL}/agui/agents/coder`, {
    method: "POST",
    headers: { "Content-Type": "application/json", Accept: "text/event-stream" },
    body: JSON.stringify({
      threadId: id,
      runId: "run-1",
      messages: [{ id: "m-1", role: "user", content: `echo ${title}` }],
    }),
  });
  expect(res.ok).toBe(true);
  await res.text();
  return id;
}

const composer = (page: Page) => page.getByRole("textbox", { name: "Message" });
const readOnly = (page: Page) => page.locator('[data-slot="read-only"]');
const scope = (page: Page) => page.getByRole("group", { name: "Whose threads" });

async function axeViolations(page: Page) {
  await animationsDone(page);
  const results = await new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa"]).analyze();
  return results.violations.filter((v) => v.impact === "serious" || v.impact === "critical");
}

test.describe("an administrator", () => {
  test("lists everyone's threads with their owners, and reads another's without being able to act", async ({
    page,
    as,
  }) => {
    const title = `roles ${uuidv7()}`;
    const id = await threadOfDev(title);
    await as("admin");
    await page.goto("/");
    await expect(agentPicker(page)).toBeVisible();
    await openThreadList(page);

    // their own list is the default, and the other is a choice with a name
    await expect(scope(page).getByRole("button", { name: "Mine" })).toHaveAttribute(
      "aria-pressed",
      "true",
    );
    await expect(threadList(page).getByRole("link", { name: new RegExp(title) })).toHaveCount(0);
    await scope(page).getByRole("button", { name: "All threads" }).click();
    await expect(scope(page).getByRole("button", { name: "All threads" })).toHaveAttribute(
      "aria-pressed",
      "true",
    );
    const row = threadList(page).getByRole("link", { name: new RegExp(title) });
    await expect(row).toBeVisible();
    await expect(row).toContainText("dev@example.com");

    await row.click();
    await expect(page).toHaveURL(new RegExp(`/threads/${id}$`));
    await expect(badge(page)).toHaveText("Done");
    // a clear line, as a status, and the chip in the top bar: words, not only a colour
    await expect(readOnly(page)).toHaveText("Read only: this is dev@example.com’s thread.");
    await expect(page.getByRole("status").filter({ hasText: /^Read only: this is/ })).toBeVisible();
    await expect(page.locator('[data-slot="read-only-chip"]')).toHaveText("Read only");
    await expect(composer(page)).toHaveCount(0);
    await expect(page.getByRole("button", { name: "Send" })).toHaveCount(0);
    await expect(conversation(page).getByText(title).first()).toBeVisible();
    await expectNoHorizontalScroll(page);

    // the menu offers the export, and not the writes
    await page.getByRole("button", { name: "Thread options" }).click();
    await expect(page.getByRole("menuitem", { name: "Rename" })).toBeDisabled();
    await expect(page.getByRole("menuitem", { name: "Export JSON" })).toBeEnabled();
    await page.keyboard.press("Escape");
    // no turn to fork, no message to edit
    await conversation(page).hover();
    await expect(page.getByRole("button", { name: "Fork from here" })).toHaveCount(0);
    await expect(page.getByRole("button", { name: "Edit what you said" })).toHaveCount(0);

    // another agent cannot continue it either: the menu says why
    await openAgentMenu(page);
    await expect(agentMenu(page).getByText(/Read only: this is dev@example.com/)).toBeVisible();
  });

  test("keeps the choice for the next page, and goes back to their own", async ({ page, as }) => {
    await as("admin");
    await page.goto("/");
    await openThreadList(page);
    await scope(page).getByRole("button", { name: "All threads" }).click();
    await page.goto("/");
    await openThreadList(page);
    await expect(scope(page).getByRole("button", { name: "All threads" })).toHaveAttribute(
      "aria-pressed",
      "true",
    );
    await scope(page).getByRole("button", { name: "Mine" }).click();
    await expect(scope(page).getByRole("button", { name: "Mine" })).toHaveAttribute(
      "aria-pressed",
      "true",
    );
  });

  test("their own threads are theirs to write in", async ({ page, as }) => {
    await as("admin");
    await page.goto("/");
    await composer(page).fill("echo mine as an administrator");
    await page.getByRole("button", { name: "Send" }).click();
    await expect(badge(page)).toHaveText("Done");
    await expect(composer(page)).toBeVisible();
    await expect(readOnly(page)).toHaveCount(0);
  });
});

test.describe("a person who is not one", () => {
  test("has no 'All threads' choice and reads only their own", async ({ page }) => {
    await page.goto("/");
    await expect(agentPicker(page)).toBeVisible();
    await openThreadList(page);
    await expect(scope(page)).toHaveCount(0);
  });

  test("a role that may not invoke an agent is offered only the ones it may", async ({
    page,
    as,
  }) => {
    await as("limited");
    await page.goto("/");
    await expect(agentPicker(page)).toHaveText("Agent: Reviewer");
    await openAgentMenu(page);
    const agents = agentMenu(page).getByRole("group", { name: "Agents" });
    await expect(agents.getByRole("menuitemradio")).toHaveCount(1);
    await expect(agents.getByRole("menuitemradio")).toContainText("Reviewer");
  });

  test("a role without thread.write is not offered a message box to start a chat", async ({
    page,
    as,
  }) => {
    await as("read-only");
    await page.goto("/");
    await expect(readOnly(page)).toHaveText("Your roles do not let you start chats.");
    await expect(composer(page)).toHaveCount(0);
  });
});

test.describe("a person whose roles grant nothing", () => {
  test("gets a screen that says who they are signed in as, not a list of errors", async ({
    page,
    as,
  }) => {
    await as("no-access");
    await page.goto("/");
    await expect(page.getByRole("heading", { level: 1, name: "No access" })).toBeVisible();
    await expect(page.getByText("nobody@example.com")).toBeVisible();
    await expect(errorLine(page)).toHaveCount(0);
    await expect(composer(page)).toHaveCount(0);
    await expectNoHorizontalScroll(page);
  });
});

for (const scheme of ["light", "dark"] as const) {
  test.describe(`accessibility (${scheme})`, () => {
    test.use({ colorScheme: scheme, contextOptions: { reducedMotion: "reduce" } });

    test("axe: a read-only thread, the administrator's list and the no-access screen have no serious violations", async ({
      page,
      as,
    }) => {
      const title = `axe ${uuidv7()}`;
      const id = await threadOfDev(title);
      await as("admin");
      await page.goto(`/threads/${id}`);
      await expect(badge(page)).toHaveText("Done");
      await expect(readOnly(page)).toBeVisible();
      await openThreadList(page);
      await scope(page).getByRole("button", { name: "All threads" }).click();
      await expect(threadList(page).getByRole("link", { name: new RegExp(title) })).toBeVisible();
      expect(await axeViolations(page)).toEqual([]);

      await as("no-access");
      await page.goto("/");
      await expect(page.getByRole("heading", { level: 1, name: "No access" })).toBeVisible();
      expect(await axeViolations(page)).toEqual([]);
    });
  });
}
