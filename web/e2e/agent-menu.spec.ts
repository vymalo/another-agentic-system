import { expect, test } from "@playwright/test";
import {
  agentMenu,
  agentMenuItem,
  agentPicker,
  badge,
  chooseAgent,
  closeAgentMenu,
  expectNoHorizontalScroll,
  openAgentMenu,
  startThread,
} from "./helpers";

/*
 * The agent picker in the top bar (web/DESIGN.md, "Agent picker"): a menu button, the same on a new
 * chat and on a thread; on a thread it names the thread's agent and offers the others as a new chat.
 */

test("the picker is in the top bar of a new chat, and names the agent that will be used", async ({
  page,
}) => {
  await page.goto("/");
  await expect(agentPicker(page)).toHaveText("Agent: Coder · production");
  await expect(agentPicker(page)).toHaveAttribute("aria-haspopup", "menu");
  await expect(agentPicker(page)).toHaveAttribute("aria-expanded", "false");
  // not in the composer any more
  await expect(page.locator('[data-slot="composer"]').getByText("Coder")).toHaveCount(0);
  await expectNoHorizontalScroll(page);
});

test("the menu lists the agents with what they do, the chosen one checked, and the keyboard drives it", async ({
  page,
}) => {
  await page.goto("/");
  await agentPicker(page).focus();
  await page.keyboard.press("Enter");
  const menu = agentMenu(page);
  await expect(menu).toBeVisible();
  const agents = menu.getByRole("group", { name: "Agents" }).getByRole("menuitemradio");
  await expect(agents).toHaveCount(3);
  await expect(agents.nth(0)).toContainText("Implements a change and opens a pull request.");
  await expect(agentMenuItem(page, "Coder")).toBeChecked();
  await expect(agentMenuItem(page, "Reviewer")).not.toBeChecked();
  await expectNoHorizontalScroll(page);

  // the arrows move, Enter chooses, the menu closes and the button says it
  await expect(agents.nth(0)).toBeFocused();
  await page.keyboard.press("ArrowDown");
  await expect(agents.nth(1)).toBeFocused();
  await page.keyboard.press("Enter");
  await expect(menu).toBeHidden();
  await expect(agentPicker(page)).toHaveText("Agent: Reviewer");
  await expect(agentPicker(page)).toBeFocused();
  // the greeting says what the chosen agent does
  await expect(page.getByText("Reviews a pull request and reports findings.")).toBeVisible();

  // Escape closes it too, and the focus goes back to the button
  await page.keyboard.press("Enter");
  await expect(menu).toBeVisible();
  await closeAgentMenu(page);
});

test("a click outside closes the menu and nothing changes", async ({ page }) => {
  await page.goto("/");
  await openAgentMenu(page);
  // the corner of the page: empty, on a phone (where the menu covers the greeting) as on a desktop
  await page.mouse.click(2, 2);
  await expect(agentMenu(page)).toBeHidden();
  await expect(agentPicker(page)).toHaveText("Agent: Coder · production");
});

test("the agent chosen in the menu is the one the first message goes to", async ({ page }) => {
  await page.goto("/");
  await chooseAgent(page, "Reviewer");
  await page.getByLabel("Message").fill("Review please");
  const request = page.waitForRequest(
    (r) => r.method() === "POST" && new URL(r.url()).pathname === "/agui/agents/reviewer",
  );
  await page.getByRole("button", { name: "Send" }).click();
  await request;
  await expect(page).toHaveURL(/\/threads\//);
  // on the thread the picker names the thread's agent
  await expect(agentPicker(page)).toHaveText("Agent: Reviewer");
});

test("on a thread the menu shows its agent and offers the others as a new chat", async ({
  page,
}) => {
  await startThread(page, "Implement the thing");
  await expect(badge(page)).toHaveText("Done");
  // the thread was started with the release the new chat showed
  await expect(agentPicker(page)).toHaveText("Agent: Coder · production");
  // the title is the page's heading, beside the picker
  await expect(page.getByRole("heading", { level: 1 })).toHaveText("Implement the thing");

  const menu = await openAgentMenu(page);
  await expect(agentMenuItem(page, "Coder")).toBeChecked();
  // the others do not change this chat: they are not radio items
  await expect(menu.getByRole("menuitemradio", { name: /^Reviewer/ })).toHaveCount(0);
  const start = menu.getByRole("group", { name: "Start a new chat with" });
  await expect(start.getByRole("menuitem")).toHaveCount(2);
  await start.getByRole("menuitem", { name: /^Reviewer/ }).click();

  // a new chat, with the reviewer already chosen
  await expect(page).toHaveURL(/\/\?agent=reviewer$/);
  await expect(agentPicker(page)).toHaveText("Agent: Reviewer");
  await expect(page.getByRole("heading", { name: "What should we get done?" })).toBeVisible();
});

test("a link to an agent that does not exist falls back to the first agent", async ({ page }) => {
  await page.goto("/?agent=nobody");
  await expect(agentPicker(page)).toHaveText("Agent: Coder · production");
});
