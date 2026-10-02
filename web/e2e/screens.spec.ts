import { expect, type Page, test } from "@playwright/test";
import { uuidv7 } from "../src/lib/uuid";
import {
  agentPicker,
  badge,
  chooseAgent,
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

// each color scheme of each device starts from an empty mock, so its sidebar holds the threads of
// its own scenarios, as many in the dark shots as in the light ones (a doc shows the pair)
async function resetMock() {
  await fetch(`${MOCK_URL}/__mock/reset`, { method: "POST" });
}

async function shot(page: Page, name: string, { hovering = false } = {}) {
  const device = test.info().project.name;
  const dark = await page.evaluate(() => matchMedia("(prefers-color-scheme: dark)").matches);
  const scheme = dark ? "dark" : "light";
  // the caret and the scroll-to-bottom button's fade are noise in a still
  // (a still of something the pointer is resting on keeps the pointer there)
  if (!hovering) await page.mouse.move(0, 0);
  await page.waitForTimeout(250);
  // a page that scrolled itself (the document, not the chat) is a still with its header cut off
  expect(await page.evaluate(() => document.scrollingElement?.scrollTop ?? 0)).toBe(0);
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
    test.beforeAll(resetMock);

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
      await expect(draft).toContainText("fix the off-by-one in the loop"); // the last piece the mock writes
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

    test("tools picker", async ({ page }) => {
      // the servers the deployment offers for the coder, two of them attached to the chat that is about to start
      await page.goto("/");
      await expect(agentPicker(page)).toBeVisible();
      await page.getByRole("button", { name: "Tools", exact: true }).click();
      await page.getByRole("menuitemcheckbox", { name: /^Web search/ }).click();
      await page.getByRole("menuitemcheckbox", { name: /^Team docs/ }).click();
      await expect(page.getByRole("menuitemcheckbox", { name: /^Team docs/ })).toBeChecked();
      await shot(page, "tools-picker");
    });

    test("tools on the steps", async ({ page }) => {
      // a chat that started with two servers: the line that says so, and in Activity each call with its server's icon
      await page.goto("/");
      await page.getByRole("button", { name: "Tools", exact: true }).click();
      await page.getByRole("menuitemcheckbox", { name: /^Web search/ }).click();
      await page.getByRole("menuitemcheckbox", { name: /^Team docs/ }).click();
      await page.keyboard.press("Escape");
      await send(page, "relay please");
      await expect(badge(page)).toHaveText("Done", { timeout: 20_000 });
      await expect(page.locator('[data-slot="tools-line"]')).toHaveText(
        "Team docs and Web search attached",
      );
      await showActivity(page);
      await expect(
        turnSections(page).first().locator('img[data-slot="step-server-icon"]'),
      ).toHaveCount(2);
      await shot(page, "tools-steps");
    });

    test("agent menu on a thread", async ({ page }) => {
      await startThread(page, "Fix the redirect loop after signing in");
      await expect(badge(page)).toHaveText("Done", { timeout: 20_000 });
      await openAgentMenu(page);
      await expect(page.getByRole("group", { name: "Agents" })).toBeVisible();
      await expect(page.getByText(/A chat keeps its agent\./)).toBeVisible();
      await shot(page, "agent-menu-thread");
    });

    test("continue with another agent", async ({ page }) => {
      // choosing another agent on a thread is a fork: the question, before anything is made
      await startThread(page, "Fix the redirect loop after signing in");
      await expect(badge(page)).toHaveText("Done", { timeout: 20_000 });
      await chooseAgent(page, "Reviewer");
      await expect(page.getByRole("alertdialog")).toBeVisible();
      await shot(page, "fork-continue");
      await page.getByRole("button", { name: "Cancel" }).click();
    });

    test("fork", async ({ page }) => {
      // "Fork from here" under the coder's answer: the new chat opens with the conversation as it
      // was, the divider that says where it was forked from, and its row in the list
      await startThread(page, "Fix the redirect loop after signing in");
      await expect(badge(page)).toHaveText("Done", { timeout: 20_000 });
      await conversation(page).getByRole("button", { name: "Fork from here" }).click();
      await expect(conversation(page).locator('[data-slot="fork-divider"]')).toBeVisible();
      await expect(badge(page)).toHaveText("Done");
      await conversation(page).locator('[data-slot="fork-divider"]').scrollIntoViewIfNeeded();
      await shot(page, "fork");
    });

    test("cards and a graph, with the panel", async ({ page }) => {
      // the answer of words, three cards and a drawn graph, in the narrower column the panel leaves
      await startThread(page, "cards-mermaid please", "Reviewer");
      await expect(badge(page)).toHaveText("Done");
      await expect(page.locator('[data-slot="mermaid-image"]')).toBeVisible();
      await page.locator('[data-slot="mermaid-image"]').scrollIntoViewIfNeeded();
      await shot(page, "cards-mermaid");
      // the answer is taller than the screen: the cards (their heading at the top) are above the graph
      await page
        .getByRole("heading", { name: "Three ways to keep a session" })
        .scrollIntoViewIfNeeded();
      await shot(page, "cards");
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

    test("step input and output: a tool step opened, and the one that failed", async ({ page }) => {
      test.setTimeout(60_000);
      await startThread(page, "steps-io please");
      await expect(badge(page)).toHaveText("Done", { timeout: 30_000 });
      await expect(turnSummaries(page)).toHaveCount(1);
      await showActivity(page);
      const turn = turnSections(page).first();
      const search = turn.getByRole("button", {
        name: /^Web search from search Stephane Segning$/,
      });
      await search.click();
      await expect(turn.locator('[data-slot="step-output"]').first()).toBeVisible();
      await turn.getByRole("button", { name: /^Failed: Command failed/ }).click();
      await expect(turn.getByRole("heading", { name: "Error" })).toBeVisible();
      // the sheet of a phone covers the chat, so the still is of the panel on both devices
      await shot(page, "step-io");
    });

    test("working text: one answer in the chat, what was said on the way in Activity", async ({
      page,
    }) => {
      test.setTimeout(60_000);
      await startThread(page, "Draw a picture in Node.js and show me the export");
      await expect(badge(page)).toHaveText("Done", { timeout: 30_000 });
      await expect(conversation(page).getByText("What the drawing holds")).toBeVisible();
      // the chat holds the surface and the answer; the sentences said before each tool call are not there
      await expect(conversation(page).locator('[data-slot="agent-message"]')).toHaveCount(1);
      // from the top of the turn: its line, the surface, the answer
      await conversation(page)
        .locator('[data-slot="turn-header"]')
        .first()
        .evaluate((el) => el.scrollIntoView({ block: "start" }));
      await shot(page, "working");
      // the sheet of a phone covers the chat, so the still is of the panel on both devices
      const tab = await showActivity(page);
      await expect(tab.locator('li[data-kind="note"]')).toHaveCount(6);
      await shot(page, "working-activity");
    });

    test("working text while the agent works: its last sentence is a quiet line", async ({
      page,
    }) => {
      test.setTimeout(60_000);
      await startThread(page, "Sketch a picture in Node.js and show me the export");
      await expect(conversation(page).locator('[data-slot="turn-ticker"]')).toHaveText(
        "All 7 tests pass. Now I'll export the drawing.",
        { timeout: 30_000 },
      );
      await expect(conversation(page).locator('[data-slot="agent-message"]')).toHaveCount(0);
      await shot(page, "working-running");
      await page.getByRole("button", { name: "Stop" }).click();
      await expect(badge(page)).toHaveText("Stopped");
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

    test("branches", async ({ page }) => {
      // the second message said again: the editor in its place, then the new chat with `‹ 2/2 ›`
      // under the new words (the conversation shows once in the list, its first thread highlighted)
      await startThread(page, "Fix the redirect loop after signing in");
      await expect(badge(page)).toHaveText("Done", { timeout: 20_000 });
      await send(page, "echo and now the tests as well");
      await expect(
        conversation(page).getByText("echo: echo and now the tests as well"),
      ).toBeVisible();
      await expect(badge(page)).toHaveText("Done");
      const second = conversation(page)
        .locator('[data-slot="user-message"]')
        .filter({ hasText: "and now the tests as well" });
      await second.hover();
      await second.getByRole("button", { name: "Edit what you said" }).click();
      const editor = page.getByRole("textbox", { name: "What you said" });
      await expect(editor).toBeFocused();
      await editor.fill("echo and now the docs as well");
      await shot(page, "branches-edit");
      await editor.press("Control+Enter");
      await expect(
        conversation(page).getByText("echo: echo and now the docs as well"),
      ).toBeVisible();
      await expect(badge(page)).toHaveText("Done");
      const edited = conversation(page)
        .locator('[data-slot="user-message"]')
        .filter({ hasText: "the docs as well" });
      await expect(edited.locator('[data-slot="branch-picker"]')).toContainText("2/2");
      await expect(edited).toBeFocused();
      await shot(page, "branches");
    });

    test("files the agent made, and the picture placed in its answer", async ({ page }) => {
      // an image, a text file and an archive: a card each (the picture and the text shown, a download beside every name)
      await startThread(page, "files make some", "Reviewer");
      await expect(badge(page)).toHaveText("Done");
      await expect(page.locator('[data-slot="file-text"]')).toContainText("Results of the run");
      const picture = page.locator('[data-slot="file-card"] [data-slot="file-image"]');
      await expect
        .poll(() => picture.evaluate((el) => (el as HTMLImageElement).naturalWidth))
        .toBeGreaterThan(0);
      // the answer's words at the top of the column, the three cards under them
      await page
        .getByText("I made three files")
        .evaluate((el) => el.scrollIntoView({ block: "start" }));
      await shot(page, "files");
      // the Sources panel lists the same files, each with a download
      if ((await panelToggle(page).getAttribute("aria-expanded")) !== "true") {
        await panelToggle(page).click();
      }
      await expect(panel(page)).toBeVisible();
      await panelTab(page, "Sources").click();
      await expect(panel(page).getByRole("region", { name: "Files" })).toBeVisible();
      await shot(page, "panel-files");
      // the catalog's Image: the same file, placed in the answer by its hash with its caption
      await startThread(page, "file-image make a chart", "Reviewer");
      await expect(badge(page)).toHaveText("Done");
      const placed = page
        .getByRole("region", { name: "Interface from reviewer" })
        .getByRole("img", { name: "A chart of the results of the run" });
      await expect
        .poll(() => placed.evaluate((el) => (el as HTMLImageElement).naturalWidth))
        .toBeGreaterThan(0);
      await page
        .getByText("I drew the chart of the results")
        .evaluate((el) => el.scrollIntoView({ block: "start" }));
      await shot(page, "file-image");
    });

    // last: the thread it makes is one more row in the list of the screens after it
    test("description: the line under the header, opened, and the hover card in the list", async ({
      page,
      isMobile,
    }) => {
      // the mock's model describes the thread (three sentences) a moment after the job ends
      await startThread(page, "Plan the session expiry test");
      await expect(badge(page)).toHaveText("Done", { timeout: 20_000 });
      const line = page.locator('[data-slot="thread-description"]');
      // the mock plays a step every 400 ms and describes the thread after the last
      await expect(line).toContainText("The person wants a plan for adding a test", {
        timeout: 20_000,
      });
      await shot(page, "description");
      await line.getByRole("button", { name: "Show more" }).click();
      await expect(line.getByRole("button", { name: "Show less" })).toBeVisible();
      await shot(page, "description-open");
      if (isMobile) return; // a phone has no hover
      await line.getByRole("button", { name: "Show less" }).click();
      await page
        .getByRole("navigation", { name: "Threads" })
        .getByRole("link", { name: "Plan the session expiry test" })
        .hover();
      await expect(page.locator('[data-slot="thread-description-card"]')).toBeVisible();
      await shot(page, "description-card", { hovering: true });
    });

    // after the description: the thread it makes belongs to the person nobody else is, and an administrator's
    // list shows it with the others
    test("roles: a thread of another's, read only, in the administrator's list of all threads, and no access", async ({
      page,
      context,
    }) => {
      const session = `screens-${uuidv7()}`;
      const mockAs = async (profile: string) => {
        await fetch(`${MOCK_URL}/__mock/config?me=${profile}&session=${session}`, {
          method: "POST",
        });
        await context.addCookies([
          { name: "mock-registry", value: session, url: "http://127.0.0.1:3000" },
        ]);
      };
      // a thread of the default person (dev@example.com), with a pull request
      const id = uuidv7();
      const made = await fetch(`${MOCK_URL}/agui/agents/coder`, {
        method: "POST",
        headers: { "Content-Type": "application/json", Accept: "text/event-stream" },
        body: JSON.stringify({
          threadId: id,
          runId: "run-1",
          messages: [
            { id: "m-1", role: "user", content: "Fix the redirect loop after signing in" },
          ],
        }),
      });
      await made.text();

      await mockAs("admin");
      await page.goto("/");
      await expect(agentPicker(page)).toBeVisible();
      await openThreadList(page);
      await page.getByRole("button", { name: "All threads" }).click();
      const row = page
        .getByRole("navigation", { name: "Threads" })
        .locator(`a[href="/threads/${id}"]`);
      await expect(row).toContainText("dev@example.com");
      await shot(page, "all-threads");
      await row.click();
      await expect(page).toHaveURL(new RegExp(`/threads/${id}$`));
      await expect(badge(page)).toHaveText("Done", { timeout: 20_000 });
      await expect(page.locator('[data-slot="read-only"]')).toBeVisible();
      await shot(page, "read-only");

      await mockAs("no-access");
      await page.goto("/");
      await expect(page.getByRole("heading", { level: 1, name: "No access" })).toBeVisible();
      await shot(page, "no-access");
    });

    // last: the threads they make are one more row in the list of the screens after them (none are)
    test("steer: the menu of the running composer, then the message sent while the agent worked", async ({
      page,
    }) => {
      // `gate …` holds the run until released: the agent works while the person writes
      await startThread(page, "gate refactor the parser");
      await expect(badge(page)).toHaveText("Working…");
      await expect(conversation(page).getByText("gate refactor the parser")).toBeVisible();
      await page.getByLabel("Message").fill("echo you were wrong since line 1");
      await page.getByRole("button", { name: "Delivery options" }).click();
      await expect(page.getByRole("menu")).toBeVisible();
      await shot(page, "steer-menu");
      await page.getByRole("menuitem", { name: /^Send/ }).click();
      await expect(page.locator('[data-slot="delivery-note"]')).toHaveText(
        "Sent while Coder was working · read at its next step",
      );
      await shot(page, "steer-sent");
      const id = /\/threads\/([0-9a-f-]{36})$/.exec(page.url())?.[1];
      await fetch(`${MOCK_URL}/__mock/release?thread=${id}`, { method: "POST" });
      await expect(badge(page)).toHaveText("Done", { timeout: 20_000 });
    });

    test("steer: Stop and send", async ({ page }) => {
      await startThread(page, "slow refactor the parser");
      await expect(badge(page)).toHaveText("Working…");
      await expect(conversation(page).getByText("slow refactor the parser")).toBeVisible();
      await page.getByLabel("Message").fill("echo do X instead");
      await page.getByLabel("Message").press("ControlOrMeta+Shift+Enter");
      await expect(page.locator('[data-slot="delivery-note"]')).toHaveText(
        "Stopped Coder · it starts again from here",
      );
      await expect(badge(page)).toHaveText("Done", { timeout: 20_000 });
      await expect(conversation(page).getByText("echo: echo do X instead")).toBeVisible();
      await shot(page, "steer-stopped");
    });
  });
}
