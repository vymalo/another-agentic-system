import AxeBuilder from "@axe-core/playwright";
import { test as base, expect, type Page } from "@playwright/test";
import { uuidv7 } from "../src/lib/uuid";
import {
  agentPicker,
  animationsDone,
  badge,
  chooseAgent,
  conversation,
  errorLine,
  expectNoHorizontalScroll,
  MOCK_URL,
  showActivity,
  THREAD_URL,
  turnSections,
} from "./helpers";

/*
 * MCP servers attached to a conversation (ADR 0024, web/DESIGN.md "Tools"), against the mock: the
 * picker in the composer's toolbar, the chips, the line in the conversation, the flag for an agent
 * that cannot use them, and the server's own icon on the steps that call it. The mock keeps the
 * deployment's servers (and who the session is) per session, named by a cookie, so each test has its
 * own list and its own person without turning the tests beside it into someone else.
 */

const ORIGIN = "http://127.0.0.1:3000";

type Server = {
  id: string;
  name: string;
  description?: string;
  icon?: string;
  agents?: string[];
};

type Deployment = {
  /** The deployment offers exactly these servers, in this order, from now on. */
  offers: (servers: Server[]) => Promise<void>;
  /** Who the session is (`user`, `admin`, `read-only`, `limited`, `no-access`). */
  as: (profile: string) => Promise<void>;
};

const test = base.extend<{ deployment: Deployment }>({
  deployment: async ({ context }, use, info) => {
    const session = `e2e-${info.workerIndex}-${info.testId}`;
    await context.addCookies([{ name: "mock-registry", value: session, url: ORIGIN }]);
    await use({
      offers: async (servers) => {
        const res = await fetch(`${MOCK_URL}/__mock/tool-servers?session=${session}`, {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify(servers),
        });
        expect(res.ok).toBe(true);
      },
      as: async (profile) => {
        const res = await fetch(`${MOCK_URL}/__mock/config?me=${profile}&session=${session}`, {
          method: "POST",
        });
        expect(res.ok).toBe(true);
      },
    });
  },
});

/** A 16 by 16 SVG as a `data:` URI: what a configuration puts in `icon`. */
const dataIcon = (fill: string) =>
  `data:image/svg+xml;base64,${Buffer.from(
    `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><rect width="16" height="16" rx="3" fill="${fill}"/></svg>`,
  ).toString("base64")}`;

const toolsButton = (page: Page) => page.getByRole("button", { name: "Tools", exact: true });
const toolItem = (page: Page, name: string) =>
  page.getByRole("menuitemcheckbox", { name: new RegExp(`^${name}`) });
const chips = (page: Page) => page.getByRole("list", { name: "Attached tools" });
const remove = (page: Page, name: string) => page.getByRole("button", { name: `Remove ${name}` });
const warning = (page: Page) => page.locator('[data-slot="tools-warning"]');
const send = (page: Page) => page.getByRole("button", { name: "Send" });

async function axeViolations(page: Page) {
  await animationsDone(page);
  const results = await new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa"]).analyze();
  return results.violations.filter((v) => v.impact === "serious" || v.impact === "critical");
}

/** Closes the open picker with Escape; the focus is back on its button. */
async function closeMenu(page: Page) {
  await page.keyboard.press("Escape");
  await expect(page.getByRole("menu")).toBeHidden();
}

/** The home page, once its agents are read: a Send before one is chosen is "Choose an agent first." */
async function home(page: Page): Promise<void> {
  await page.goto("/");
  await expect(agentPicker(page)).toBeVisible();
}

/** A thread of the session's person, started from the home page and run to its end. */
async function finishedThread(page: Page, text: string): Promise<void> {
  await home(page);
  await page.getByLabel("Message").fill(text);
  await send(page).click();
  await expect(page).toHaveURL(THREAD_URL);
  await expect(badge(page)).toHaveText("Done", { timeout: 30_000 });
}

test("a new chat: pick Web search and Team docs, send, and the thread starts with them", async ({
  page,
  deployment,
}) => {
  void deployment;
  const runs: Record<string, unknown>[] = [];
  page.on("request", (req) => {
    if (req.method() === "POST" && new URL(req.url()).pathname.startsWith("/agui/agents/")) {
      runs.push(req.postDataJSON() as Record<string, unknown>);
    }
  });
  await page.goto("/");
  await expect(toolsButton(page)).toBeVisible();
  await expect(chips(page)).toHaveCount(0);
  await expectNoHorizontalScroll(page);

  await toolsButton(page).click();
  const menu = page.getByRole("menu");
  await expect(menu).toBeVisible();
  // the servers of the deployment for the coder, in its order, each with what it is for
  await expect(menu.getByRole("menuitemcheckbox")).toHaveText([
    /^Web searchSearch the web and read what comes back\./,
    /^GitHubRead repositories, issues and pull requests\./,
    /^Team docsLook things up in the team's documentation\./,
  ]);
  await toolItem(page, "Web search").click();
  // a choice does not close the menu: several are made in one visit
  await expect(menu).toBeVisible();
  await expect(toolItem(page, "Web search")).toBeChecked();
  await toolItem(page, "Team docs").click();
  await expect(toolItem(page, "Team docs")).toBeChecked();
  await closeMenu(page);
  await expect(toolsButton(page)).toBeFocused();

  // the choice, as removable chips, in the deployment's own words
  await expect(chips(page).getByRole("listitem")).toHaveText(["Team docs", "Web search"]);
  await expect(warning(page)).toHaveCount(0);
  await expectNoHorizontalScroll(page);

  // a chip can be taken off before sending
  await remove(page, "Team docs").click();
  await expect(chips(page).getByRole("listitem")).toHaveText(["Web search"]);
  await toolsButton(page).click();
  await expect(toolItem(page, "Team docs")).not.toBeChecked();
  await closeMenu(page);

  await page.getByLabel("Message").fill("echo with tools");
  await send(page).click();
  await expect(page).toHaveURL(THREAD_URL);
  // the run that creates the thread carries the ids, and nothing else of the servers
  const created = runs[0]?.forwardedProps as Record<string, unknown>;
  expect(created["vymalo.tools"]).toEqual(["websearch"]);

  // the conversation says it, with the name from the list, and the thread keeps the chip
  await expect(badge(page)).toHaveText("Done", { timeout: 30_000 });
  await expect(page.locator('[data-slot="tools-line"]')).toHaveText("Web search attached");
  await expect(chips(page).getByRole("listitem")).toHaveText(["Web search"]);
  // a follow-up carries no `vymalo.tools`: the set is the thread's
  await page.getByLabel("Message").fill("echo second");
  await send(page).click();
  await expect(badge(page)).toHaveText("Done", { timeout: 30_000 });
  expect(runs).toHaveLength(2);
  expect(Object.keys(runs[1]?.forwardedProps as Record<string, unknown>)).not.toContain(
    "vymalo.tools",
  );
  await expect(errorLine(page)).toHaveCount(0);
});

test("a thread that exists: attach, detach, in any state, and the same after a reload", async ({
  page,
  deployment,
}) => {
  void deployment;
  await finishedThread(page, "echo nothing yet");
  await expect(chips(page)).toHaveCount(0);
  await expect(page.locator('[data-slot="tools-line"]')).toHaveCount(0);

  await toolsButton(page).click();
  await toolItem(page, "Team docs").click();
  await expect(toolItem(page, "Team docs")).toBeChecked();
  await closeMenu(page);
  await expect(chips(page).getByRole("listitem")).toHaveText(["Team docs"]);
  // the log says it, as a line of the conversation (the thread was finished: a run of its own)
  await expect(page.locator('[data-slot="tools-line"]')).toHaveText("Team docs attached");
  await expect(badge(page)).toHaveText("Done");

  // after a reload the thread still has it: the resource and the replay say the same
  await page.reload();
  await expect(chips(page).getByRole("listitem")).toHaveText(["Team docs"]);
  await expect(page.locator('[data-slot="tools-line"]')).toHaveText("Team docs attached");

  // a second one, then both off: each change is its own line, in the order they happened
  await toolsButton(page).click();
  await toolItem(page, "Web search").click();
  await expect(toolItem(page, "Web search")).toBeChecked();
  await closeMenu(page);
  await expect(chips(page).getByRole("listitem")).toHaveText(["Team docs", "Web search"]);
  await expect(page.locator('[data-slot="tools-line"]')).toHaveText([
    "Team docs attached",
    "Web search attached",
  ]);

  await remove(page, "Team docs").click();
  await expect(chips(page).getByRole("listitem")).toHaveText(["Web search"]);
  await remove(page, "Web search").click();
  await expect(chips(page)).toHaveCount(0);
  await expect(page.locator('[data-slot="tools-line"]')).toHaveText([
    "Team docs attached",
    "Web search attached",
    "Team docs detached",
    "Web search detached",
  ]);
  await toolsButton(page).click();
  await expect(page.getByRole("menuitemcheckbox", { checked: true })).toHaveCount(0);
  await closeMenu(page);

  await page.reload();
  await expect(chips(page)).toHaveCount(0);
  await expect(errorLine(page)).toHaveCount(0);
  await expectNoHorizontalScroll(page);
});

test("a thread that is working takes a server too, and the line comes in its turn", async ({
  page,
  deployment,
}) => {
  void deployment;
  await home(page);
  await page.getByLabel("Message").fill("Refactor the module");
  await send(page).click();
  await expect(page).toHaveURL(THREAD_URL);
  await expect(badge(page)).toHaveText("Working…");
  await toolsButton(page).click();
  await toolItem(page, "Web search").click();
  await expect(toolItem(page, "Web search")).toBeChecked();
  await closeMenu(page);
  await expect(chips(page).getByRole("listitem")).toHaveText(["Web search"]);
  await expect(page.locator('[data-slot="tools-line"]')).toHaveText("Web search attached");
  await page.getByRole("button", { name: "Stop" }).click();
  await expect(badge(page)).toHaveText("Stopped", { timeout: 30_000 });
});

test("an agent whose card does not list thread-tools/v1 is flagged before anything is sent", async ({
  page,
  deployment,
}) => {
  void deployment;
  await home(page);
  await toolsButton(page).click();
  await toolItem(page, "Web search").click();
  await closeMenu(page);
  // the coder lists the extension: no flag
  await expect(chips(page).getByRole("listitem")).toHaveText(["Web search"]);
  await expect(warning(page)).toHaveCount(0);

  // the reviewer does not: said at once, before a message exists, and the choice is kept
  await chooseAgent(page, "Reviewer");
  await expect(warning(page)).toHaveText(
    "Reviewer cannot use attached tools, so they will not be sent to it.",
  );
  await expect(chips(page).getByRole("listitem")).toHaveText(["Web search"]);
  await expectNoHorizontalScroll(page);
  // the menu says it too, and offers the servers that the reviewer may have (not GitHub)
  await toolsButton(page).click();
  await expect(page.getByRole("menuitemcheckbox")).toHaveText([/^Web search/, /^Team docs/]);
  await expect(page.locator('[data-slot="tools-hint"]')).toHaveText(
    "This agent does not use attached tools.",
  );
  await closeMenu(page);

  // back to the coder: the flag goes
  await chooseAgent(page, "Adam");
  await expect(warning(page)).toHaveCount(0);

  // a thread of the reviewer says it too, where the tools are attached
  await chooseAgent(page, "Reviewer");
  await remove(page, "Web search").click();
  await expect(warning(page)).toHaveCount(0);
  await page.getByLabel("Message").fill("echo reviewed");
  await send(page).click();
  await expect(page).toHaveURL(THREAD_URL);
  await expect(badge(page)).toHaveText("Done", { timeout: 30_000 });
  await expect(warning(page)).toHaveCount(0);
  await toolsButton(page).click();
  await toolItem(page, "Web search").click();
  await closeMenu(page);
  await expect(warning(page)).toHaveText(
    "Reviewer cannot use attached tools, so they will not be sent to it.",
  );
  await remove(page, "Web search").click();
  await expect(warning(page)).toHaveCount(0);
});

test("a server the agent may not use is not offered, and a deployment with nothing to offer has no picker", async ({
  page,
  deployment,
}) => {
  await deployment.offers([{ id: "github", name: "GitHub", agents: ["adam"] }]);
  await page.goto("/");
  await expect(toolsButton(page)).toBeVisible();
  await chooseAgent(page, "Reviewer");
  // nothing for the reviewer, and nothing attached: no picker at all
  await expect(toolsButton(page)).toHaveCount(0);
  await chooseAgent(page, "Adam");
  await expect(toolsButton(page)).toBeVisible();

  await deployment.offers([]);
  await page.goto("/");
  await expect(page.getByLabel("Message")).toBeVisible();
  await expect(toolsButton(page)).toHaveCount(0);
  await expect(chips(page)).toHaveCount(0);
});

test("the icon is drawn from a data: URI and from nothing else: an http(s) icon is never requested", async ({
  page,
  deployment,
}) => {
  const tracker = `${MOCK_URL}/__mock/probe/icon.svg`;
  await deployment.offers([
    { id: "websearch", name: "Web search", icon: dataIcon("#2f6f4f") },
    { id: "github", name: "GitHub" },
    { id: "docs", name: "Team docs", icon: "https://tracker.example/docs.png" },
    { id: "files", name: "Files", icon: tracker },
    { id: "wiki", name: "Wiki", icon: "//tracker.example/wiki.svg" },
    { id: "mail", name: "Mail", icon: "data:text/html;base64,PGgxPmhpPC9oMT4=" },
  ]);
  const asked: string[] = [];
  page.on("request", (req) => asked.push(req.url()));
  // a request that did get out would not have an answer either
  await page.route(/tracker\.example|__mock\/probe/, (route) => route.abort());

  await home(page);
  await toolsButton(page).click();
  const menu = page.getByRole("menu");
  await expect(menu.getByRole("menuitemcheckbox")).toHaveCount(6);
  // the one icon that is a data: image is an <img>; every other server has the generic icon
  const images = menu.locator('img[data-slot="server-icon"]');
  await expect(images).toHaveCount(1);
  expect(await images.first().getAttribute("src")).toBe(dataIcon("#2f6f4f"));
  await expect(images.first()).toHaveJSProperty("complete", true);
  expect(await images.first().evaluate((el: HTMLImageElement) => el.naturalWidth)).toBeGreaterThan(
    0,
  );
  await expect(menu.locator('svg[data-slot="server-icon"]')).toHaveCount(5);
  await toolItem(page, "Team docs").click();
  await toolItem(page, "Files").click();
  await toolItem(page, "Web search").click();
  await closeMenu(page);
  // the chips draw the same way
  await expect(chips(page).locator('img[data-slot="server-icon"]')).toHaveCount(1);
  await expect(chips(page).locator('svg[data-slot="server-icon"]')).toHaveCount(2);

  // the steps of a conversation that called these servers: the same rule in the panel
  await page.getByLabel("Message").fill("relay please");
  await send(page).click();
  await expect(page).toHaveURL(THREAD_URL);
  await expect(badge(page)).toHaveText("Done", { timeout: 30_000 });
  await showActivity(page);
  const turn = turnSections(page).first();
  const search = turn.locator('[data-step="T/tool:r1"]');
  await expect(search).toContainText("Web search · search");
  const icon = search.locator('img[data-slot="step-server-icon"]');
  await expect(icon).toHaveCount(1);
  expect(await icon.getAttribute("src")).toBe(dataIcon("#2f6f4f"));
  // a server the list has no icon for, one with a URL for an icon and one the list does not have:
  // the glyph of a tool, never an image
  for (const id of ["tool:r2", "tool:r3", "tool:r4"]) {
    await expect(turn.locator(`[data-step="T/${id}"]`)).toBeVisible();
    await expect(turn.locator(`[data-step="T/${id}"] img`)).toHaveCount(0);
  }
  await expect(turn.locator('img[data-slot="step-server-icon"]')).toHaveCount(1);
  for (const src of await page
    .locator("img")
    .evaluateAll((els) => els.map((e) => e.getAttribute("src")))) {
    if (src !== null && !src.startsWith("/")) expect(src).toMatch(/^data:image\//);
  }

  // after all of it, nothing was asked of any icon's address
  expect(asked.filter((url) => /tracker\.example|__mock\/probe/.test(url))).toEqual([]);
  expect(asked.filter((url) => url.startsWith("http") && !url.startsWith(ORIGIN))).toEqual([]);
});

test("the read-only view has no picker: a role that writes nothing", async ({
  page,
  deployment,
}) => {
  // a thread made by the default person, then handed to the person who reads and does not write
  const id = uuidv7();
  const made = await fetch(`${MOCK_URL}/agui/agents/adam`, {
    method: "POST",
    headers: { "Content-Type": "application/json", Accept: "text/event-stream" },
    body: JSON.stringify({
      threadId: id,
      runId: "run-1",
      forwardedProps: { "vymalo.tools": ["websearch"] },
      messages: [{ id: "m-1", role: "user", content: "echo theirs" }],
    }),
  });
  expect(made.ok).toBe(true);
  await made.text();
  const handed = await fetch(`${MOCK_URL}/__mock/owner?thread=${id}&owner=viewer@example.com`, {
    method: "POST",
  });
  expect(handed.status).toBe(204);

  await deployment.as("read-only");
  await page.goto(`/threads/${id}`);
  await expect(page.locator('[data-slot="read-only"]')).toContainText("Read only: your roles");
  await expect(toolsButton(page)).toHaveCount(0);
  await expect(chips(page)).toHaveCount(0);
  // what happened is still said; listing the servers takes `thread.write`, so the line names the id
  await expect(page.locator('[data-slot="tools-line"]')).toHaveText("websearch attached");

  await page.goto("/");
  await expect(page.locator('[data-slot="read-only"]')).toBeVisible();
  await expect(toolsButton(page)).toHaveCount(0);
  // no `thread.write`: the page does not even ask for the list
  const asked: string[] = [];
  page.on("request", (req) => asked.push(new URL(req.url()).pathname));
  await page.reload();
  await expect(page.locator('[data-slot="read-only"]')).toBeVisible();
  await page.waitForLoadState("networkidle");
  expect(asked).not.toContain("/api/tool-servers");
});

test("the keyboard alone: open the picker, choose, close, take a chip off, send", async ({
  page,
  deployment,
}) => {
  void deployment;
  await page.goto("/");
  const message = page.getByLabel("Message");
  // the picker is there once the list is read
  await expect(toolsButton(page)).toBeVisible();
  await message.focus();
  // the toolbar follows the box in the tab order
  await page.keyboard.press("Tab");
  await expect(toolsButton(page)).toBeFocused();
  await page.keyboard.press("Enter");
  await expect(page.getByRole("menu")).toBeVisible();
  // opened from the keyboard, the menu puts the focus on its first item
  await expect(toolItem(page, "Web search")).toBeFocused();
  await page.keyboard.press("Space");
  await expect(toolItem(page, "Web search")).toBeChecked();
  // the menu stays open for the next one
  await page.keyboard.press("ArrowDown");
  await expect(toolItem(page, "GitHub")).toBeFocused();
  await page.keyboard.press("ArrowDown");
  await expect(toolItem(page, "Team docs")).toBeFocused();
  await page.keyboard.press("Enter");
  await expect(toolItem(page, "Team docs")).toBeChecked();
  await page.keyboard.press("Escape");
  await expect(page.getByRole("menu")).toBeHidden();
  await expect(toolsButton(page)).toBeFocused();
  await expect(chips(page).getByRole("listitem")).toHaveText(["Team docs", "Web search"]);

  // the chips' buttons are in the tab order, and Enter takes one off
  await page.keyboard.press("Tab");
  await expect(remove(page, "Team docs")).toBeFocused();
  await page.keyboard.press("Enter");
  await expect(chips(page).getByRole("listitem")).toHaveText(["Web search"]);

  // on to Send
  await message.fill("echo keyboard");
  await message.focus();
  await page.keyboard.press("Enter");
  await expect(page).toHaveURL(THREAD_URL);
  await expect(page.locator('[data-slot="tools-line"]')).toHaveText("Web search attached");
  await expect(badge(page)).toHaveText("Done", { timeout: 30_000 });
});

test("a refused change is said in the orchestrator's words, and the chip is not drawn", async ({
  page,
  deployment,
}) => {
  void deployment;
  await finishedThread(page, "echo refusal");
  await page.route("**/api/threads/*/tools", (route) =>
    route.fulfill({
      status: 422,
      contentType: "application/problem+json",
      body: JSON.stringify({
        title: "Unprocessable",
        status: 422,
        detail: "the server websearch is not offered for the agent coder",
      }),
    }),
  );
  await toolsButton(page).click();
  await toolItem(page, "Web search").click();
  await expect(errorLine(page)).toHaveText(
    "the server websearch is not offered for the agent coder",
  );
  await expect(chips(page)).toHaveCount(0);
  await expect(toolItem(page, "Web search")).not.toBeChecked();
});

for (const scheme of ["light", "dark"] as const) {
  test.describe(`accessibility (${scheme})`, () => {
    // a turn fades in for 160 ms: axe would read colours that are not at rest yet
    test.use({ colorScheme: scheme, contextOptions: { reducedMotion: "reduce" } });

    test("axe: the picker closed with chips, open, and with the flag for an agent that cannot use them", async ({
      page,
      deployment,
    }) => {
      await deployment.offers([
        {
          id: "websearch",
          name: "Web search",
          description: "Search the web.",
          icon: dataIcon("#2f6f4f"),
        },
        { id: "github", name: "GitHub", description: "Read repositories." },
        { id: "docs", name: "Team docs", agents: ["adam", "reviewer"] },
      ]);
      await home(page);
      await toolsButton(page).click();
      await toolItem(page, "Web search").click();
      await toolItem(page, "GitHub").click();
      await expect(page.getByRole("menu")).toBeVisible();
      expect(await axeViolations(page)).toEqual([]);
      await closeMenu(page);
      await expect(chips(page).getByRole("listitem")).toHaveCount(2);
      expect(await axeViolations(page)).toEqual([]);

      await chooseAgent(page, "Reviewer");
      await expect(warning(page)).toBeVisible();
      expect(await axeViolations(page)).toEqual([]);
      await toolsButton(page).click();
      await expect(page.getByRole("menu")).toBeVisible();
      expect(await axeViolations(page)).toEqual([]);
      await closeMenu(page);
    });

    test("axe: a thread with tools attached, its line, and the steps that call them in the panel", async ({
      page,
      deployment,
    }) => {
      await deployment.offers([
        { id: "websearch", name: "Web search", icon: dataIcon("#2f6f4f") },
        { id: "github", name: "GitHub" },
        { id: "docs", name: "Team docs" },
      ]);
      await home(page);
      await toolsButton(page).click();
      await toolItem(page, "Web search").click();
      await toolItem(page, "GitHub").click();
      await closeMenu(page);
      await page.getByLabel("Message").fill("relay please");
      await send(page).click();
      await expect(page).toHaveURL(THREAD_URL);
      await expect(badge(page)).toHaveText("Done", { timeout: 30_000 });
      await expect(page.locator('[data-slot="tools-line"]')).toHaveText(
        "GitHub and Web search attached",
      );
      await expect(conversation(page)).toBeVisible();
      expect(await axeViolations(page)).toEqual([]);
      await showActivity(page);
      await expect(
        turnSections(page).first().locator('img[data-slot="step-server-icon"]'),
      ).toHaveCount(1);
      expect(await axeViolations(page)).toEqual([]);
    });
  });
}
