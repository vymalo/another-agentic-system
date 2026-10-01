import { expect, type Page, test } from "@playwright/test";
import {
  agentPicker,
  badge,
  conversation,
  MOCK_URL,
  openAgentMenu,
  openThreadList,
  panel,
  panelTab,
  panelToggle,
  showActivity,
  startThread,
  turnSections,
  turnSummaries,
} from "./helpers";

/*
 * `pnpm screens`: the screenshots of every state a person meets, for both color schemes, on a
 * desktop (1440×900) and a phone (390×844), written to e2e/__screens__/<device>-<scheme>-<state>.png.
 * They are for reading, not asserting: each waits for its state, then takes the viewport. The
 * scenarios are the mock's coder scripts (mock/scripts.ts, `Fix …`, `Make …` and so on).
 */

const DIR = "e2e/__screens__";

// each device starts from an empty mock, so its sidebar holds the threads of its own scenarios
test.beforeAll(async () => {
  await fetch(`${MOCK_URL}/__mock/reset`, { method: "POST" });
});

async function shot(page: Page, name: string) {
  const device = test.info().project.name;
  const dark = await page.evaluate(() => matchMedia("(prefers-color-scheme: dark)").matches);
  const scheme = dark ? "dark" : "light";
  // the caret and the scroll-to-bottom button's fade are noise in a still
  await page.mouse.move(0, 0);
  await page.waitForTimeout(250);
  await page.screenshot({
    path: `${DIR}/${device}-${scheme}-${name}.png`,
    animations: "disabled",
    caret: "hide",
    // one pixel per CSS pixel keeps the files small; the phone renders at 2x and is scaled down
    scale: "css",
  });
}

async function send(page: Page, text: string) {
  await page.getByLabel("Message").fill(text);
  await page.getByRole("button", { name: "Send" }).click();
}

for (const scheme of ["light", "dark"] as const) {
  test.describe(scheme, () => {
    test.use({ colorScheme: scheme });

    test("empty thread", async ({ page }) => {
      await page.goto("/");
      await expect(agentPicker(page)).toBeVisible();
      await shot(page, "empty-thread");
    });

    test("starting", async ({ page }) => {
      await startThread(page, "Upgrade the dependencies to their latest minor versions");
      await expect(badge(page)).toBeVisible();
      await page.waitForTimeout(600);
      await shot(page, "starting");
      await page.getByRole("button", { name: "Stop" }).click();
      await expect(badge(page)).toHaveText("Stopped");
    });

    test("agent working", async ({ page }) => {
      await startThread(page, "Refactor the session store behind a trait");
      await expect(conversation(page).getByText("cargo test -p auth login::")).toBeVisible();
      await shot(page, "agent-working");
      await page.getByRole("button", { name: "Stop" }).click();
      await expect(badge(page)).toHaveText("Stopped");
    });

    test("reply writing", async ({ page }) => {
      // the words of the reply as the agent writes them: a draft after the turn's parts, with its caret
      await startThread(page, "Write the plan for the login redirect");
      const draft = conversation(page).locator('[data-slot="agent-draft"]');
      await expect(draft).toContainText("then make the smallest change");
      await shot(page, "reply-writing");
      await page.getByRole("button", { name: "Stop" }).click();
      await expect(badge(page)).toHaveText("Stopped");
    });

    test("your turn", async ({ page }) => {
      await startThread(page, "Deploy the new login page");
      await expect(badge(page)).toHaveText("Your turn");
      await shot(page, "your-turn");
    });

    test("done with a pull request", async ({ page }) => {
      await startThread(page, "Fix the redirect loop after signing in");
      await expect(badge(page)).toHaveText("Done", { timeout: 20_000 });
      await shot(page, "done-pull-request");
      await page.getByText("I fixed the redirect loop").scrollIntoViewIfNeeded();
      await shot(page, "done-answer");
    });

    test("follow-up after done", async ({ page }) => {
      await startThread(page, "Fix the redirect loop after signing in");
      await expect(badge(page)).toHaveText("Done", { timeout: 20_000 });
      await send(page, "Also add an entry to the changelog");
      await expect(conversation(page).getByText("Added a changelog entry")).toBeVisible({
        timeout: 20_000,
      });
      await expect(badge(page)).toHaveText("Done");
      await shot(page, "follow-up");
    });

    test("rework with failed checks", async ({ page }) => {
      test.setTimeout(60_000);
      await startThread(page, "Make sessions expire after 30 idle minutes");
      await expect(badge(page)).toHaveText("Done", { timeout: 30_000 });
      await shot(page, "rework-done");
      // the rework is a step of the turn: the panel's (a sheet on a phone)
      const tab = await showActivity(page);
      await tab.getByText(/trying again/).scrollIntoViewIfNeeded();
      await shot(page, "rework");
    });

    test("error", async ({ page }) => {
      await startThread(page, "Migrate the legacy repository to the new CI");
      await expect(badge(page)).toHaveText("Failed", { timeout: 20_000 });
      await shot(page, "error");
    });

    test("agent menu", async ({ page }) => {
      await page.goto("/");
      await openAgentMenu(page);
      await expect(page.getByRole("menuitemradio", { name: /^production/ })).toBeChecked();
      await shot(page, "agent-menu");
    });

    test("agent menu on a thread", async ({ page }) => {
      await startThread(page, "Fix the redirect loop after signing in");
      await expect(badge(page)).toHaveText("Done", { timeout: 20_000 });
      await openAgentMenu(page);
      await expect(page.getByRole("group", { name: "Start a new chat with" })).toBeVisible();
      await shot(page, "agent-menu-thread");
    });

    test("cards and a graph, with the panel", async ({ page }) => {
      // the answer of words, three cards and a drawn graph, in the narrower column the panel leaves
      await startThread(page, "cards-mermaid please", "Reviewer");
      await expect(badge(page)).toHaveText("Done");
      await expect(page.locator('[data-slot="mermaid-image"]')).toBeVisible();
      await page.locator('[data-slot="mermaid-image"]').scrollIntoViewIfNeeded();
      await shot(page, "cards-mermaid");
    });

    test("panel", async ({ page }) => {
      // beside the chat on a desktop (open by itself on a wide window), a sheet from the bottom on a phone
      await startThread(page, "sources the login redirect");
      await expect(badge(page)).toHaveText("Done", { timeout: 20_000 });
      if ((await panelToggle(page).getAttribute("aria-expanded")) !== "true") {
        await panelToggle(page).click();
      }
      await expect(panel(page)).toBeVisible();
      await shot(page, "panel-activity");
      await panelTab(page, "Sources").click();
      await expect(panel(page).getByRole("region", { name: "Links" })).toBeVisible();
      await shot(page, "panel-sources");
    });

    test("steps: a delegation to OpenCode, one line in the chat and the tree in the panel", async ({
      page,
    }) => {
      test.setTimeout(60_000);
      await startThread(page, "Delegate the login fix to OpenCode");
      await expect(badge(page)).toHaveText("Done", { timeout: 30_000 });
      await expect(turnSummaries(page)).toHaveCount(1);
      // the conversation: the line, the answer, the pull request (the panel docked beside it on a desktop)
      await shot(page, "steps-chat");
      await turnSummaries(page).click();
      const turn = turnSections(page).first();
      await expect(turn.getByRole("heading", { level: 3 })).toBeFocused();
      await shot(page, "steps-collapsed");
      await turn.getByRole("button", { name: /^OpenCode/ }).click();
      await expect(turn.getByRole("list", { name: "Steps of OpenCode" })).toBeVisible();
      await shot(page, "steps-opened");
      await turn.getByRole("button", { name: /^Show 10 more steps of OpenCode/ }).click();
      await expect(
        turn.getByRole("list", { name: "Steps of OpenCode" }).getByRole("listitem"),
      ).toHaveCount(13);
      await shot(page, "steps-more");
    });

    test("steps while the agent works", async ({ page }) => {
      await startThread(page, "Investigate the login redirect");
      await expect(turnSummaries(page)).toContainText("Running cargo test -p auth");
      await expect(turnSummaries(page).locator('[data-glyph="spinner"]')).toBeVisible();
      await page.waitForTimeout(400);
      await shot(page, "steps-running");
      if (await panelToggle(page).isVisible()) {
        if ((await panelToggle(page).getAttribute("aria-expanded")) !== "true") {
          await panelToggle(page).click();
        }
        await expect(panel(page)).toBeVisible();
        await expect(turnSections(page).first()).toBeVisible();
        await shot(page, "steps-running-panel");
        if (await page.getByRole("dialog", { name: "Thread details" }).isVisible()) {
          await page.keyboard.press("Escape");
        }
      }
      await page.getByRole("button", { name: "Stop" }).click();
      await expect(badge(page)).toHaveText("Stopped");
    });

    test("sidebar", async ({ page, isMobile }) => {
      await page.goto("/");
      await expect(agentPicker(page)).toBeVisible();
      if (isMobile) {
        await openThreadList(page);
        await expect(page.getByRole("dialog", { name: "Threads" })).toBeVisible();
      } else {
        await page.getByRole("button", { name: "Close sidebar" }).click();
        await expect(page.getByRole("button", { name: "Open sidebar" })).toBeVisible();
      }
      await shot(page, "sidebar");
    });
  });
}
