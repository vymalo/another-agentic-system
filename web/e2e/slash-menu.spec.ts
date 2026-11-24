import AxeBuilder from "@axe-core/playwright";
import { test as base, expect, type Page } from "@playwright/test";
import {
  animationsDone,
  conversation,
  expectNoHorizontalScroll,
  MOCK_URL,
  THREAD_URL,
} from "./helpers";

/*
 * The one `/` menu of the message box (ADR 0067 slice P3): the agent's own commands (ADR 0079,
 * docs/api/commands-v1.md section 6) and the person's prompts (docs/api/prompts-v1.md) as two
 * labelled groups of **one** list, commands first. Picking keeps each path as it was — a command
 * inserts `/name ` (and the message that goes out is that text), a prompt opens its form and puts
 * the rendered text in the box — and neither sends anything.
 *
 * Everything here is the web's mock server: the agent's card lists its commands through
 * `/__mock/commands`, and the person's library is seeded through the mock's own `/api/prompts`.
 */

const ORIGIN = "http://127.0.0.1:3000";
const COMMANDS = {
  commands: [
    { name: "review", description: "Reviews a change.", argumentHint: "<repo>" },
    { name: "explain", description: "Explains a file." },
  ],
};

type Session = {
  /** What the card of `agent` lists under `commands/v1` from now on (`null`: nothing). */
  lists: (agent: string, params: unknown) => Promise<void>;
  /** Writes a prompt into this session's library, as `POST /api/prompts` does. */
  prompt: (body: unknown) => Promise<{ id: string; revision: string; name: string }>;
};

/** The mock keeps the cards and libraries per session, named by a cookie: a test is its own. */
const test = base.extend<{ session: Session }>({
  session: async ({ context, request }, use, info) => {
    const name = `e2e-slash-${info.workerIndex}-${info.testId}-${info.repeatEachIndex}`;
    await context.addCookies([{ name: "mock-registry", value: name, url: ORIGIN }]);
    const hook = async (path: string, data?: unknown) => {
      const res = await request.post(
        `${MOCK_URL}/__mock/${path}${path.includes("?") ? "&" : "?"}session=${name}`,
        data === undefined ? {} : { data },
      );
      expect(res.ok()).toBe(true);
    };
    await use({
      lists: (agent, params) => hook("commands", { agent, params }),
      prompt: async (body) => {
        const res = await fetch(`${MOCK_URL}/api/prompts`, {
          method: "POST",
          headers: { "Content-Type": "application/json", Cookie: `mock-registry=${name}` },
          body: JSON.stringify(body),
        });
        expect(res.status, await res.clone().text()).toBe(201);
        return (await res.json()) as { id: string; revision: string; name: string };
      },
    });
  },
});

const box = (page: Page) => page.getByLabel("Message", { exact: true });
/** The one menu, and its two groups (the listbox itself is absent when there is no row to draw). */
const menu = (page: Page) => page.locator('[data-slot="slash-menu"]');
const commandsGroup = (page: Page) => page.getByRole("group", { name: "Agent commands" });
const promptsGroup = (page: Page) => page.getByRole("group", { name: "Your prompts" });
const commandRows = (page: Page) => commandsGroup(page).getByRole("option");
const promptRows = (page: Page) => promptsGroup(page).getByRole("option");

/** The bodies of the runs the page posted, as they go out. */
function posted(page: Page) {
  const bodies: { messages: { content: string }[] }[] = [];
  page.on("request", (r) => {
    if (r.method() === "POST" && /\/agui\/agents\//.test(r.url())) {
      bodies.push(r.postDataJSON() as (typeof bodies)[number]);
    }
  });
  return bodies;
}

async function axeViolations(page: Page) {
  await animationsDone(page);
  const results = await new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa"]).analyze();
  return results.violations.filter((v) => v.impact === "serious" || v.impact === "critical");
}

/** A prompt with one question, so that a pick opens the fill form. */
const reviewPrompt = {
  name: "review",
  title: "Review a change",
  description: "Reviews a change for one concern",
  body: "Review for {{focus}}.",
  variables: [{ id: "focus", question: "What focus?", type: "text" }],
};

test("one / lists the agent's commands and the person's prompts, and a pick of each does what it did", async ({
  page,
  session,
}) => {
  await session.lists("adam", COMMANDS);
  await session.prompt(reviewPrompt);
  const bodies = posted(page);
  await page.goto("/");
  await expect(box(page)).toBeVisible();

  // a bare slash: one list, two labelled groups, the agent's first (the epic's order)
  await box(page).fill("/");
  await expect(menu(page)).toBeVisible();
  await expect(page.getByRole("listbox", { name: "Commands and prompts" })).toBeVisible();
  await expect(commandsGroup(page)).toBeVisible();
  await expect(promptsGroup(page)).toBeVisible();
  const labels = page.getByText(/^(Agent commands|Your prompts)$/);
  await expect(labels).toHaveCount(2);
  expect(await labels.allTextContents()).toEqual(["Agent commands", "Your prompts"]);
  await expect(commandRows(page)).toHaveCount(2);
  await expect(promptRows(page)).toHaveCount(1);
  await expect(commandRows(page).nth(0)).toContainText("/explain");
  await expect(commandRows(page).nth(1)).toContainText("<repo>");
  await expect(promptRows(page).nth(0)).toContainText("/review");
  await expect(promptRows(page).nth(0)).toContainText("Review a change");
  // the focus stays in the box, which names the first row of the first group
  await expect(box(page)).toBeFocused();
  await expect(box(page)).toHaveAttribute(
    "aria-activedescendant",
    (await commandRows(page).nth(0).getAttribute("id")) ?? "",
  );

  // the word both halves hold: two rows, the command first
  await box(page).fill("/rev");
  await expect(commandRows(page)).toHaveCount(1);
  await expect(promptRows(page)).toHaveCount(1);
  await page.keyboard.press("Enter");
  await expect(box(page)).toHaveValue("/review ");
  await expect(menu(page)).toHaveCount(0);
  expect(bodies).toHaveLength(0);

  // the arguments are typed after the space, and the message that goes out is that text
  await page.keyboard.type("acme/widgets");
  await expect(box(page)).toHaveValue("/review acme/widgets");
  await page.getByRole("button", { name: "Send" }).click();
  await expect(page).toHaveURL(THREAD_URL);
  expect(bodies[0]?.messages[0]?.content).toBe("/review acme/widgets");
  await expect(conversation(page).getByText("echo: /review acme/widgets")).toBeVisible();
  await expectNoHorizontalScroll(page);

  // a pick of the prompt opens its form, as it did before (here on the same new chat)
  await page.goto("/");
  await box(page).fill("/rev");
  await expect(promptRows(page)).toHaveCount(1);
  await page.keyboard.press("ArrowDown");
  await expect(promptRows(page).nth(0)).toHaveAttribute("aria-selected", "true");
  await page.keyboard.press("Enter");
  const dialog = page.getByRole("dialog");
  await expect(dialog).toBeVisible();
  await expect(dialog.getByRole("heading", { name: "Review a change" })).toBeVisible();
  await expect(box(page)).toHaveValue("");
  await dialog.getByRole("textbox", { name: "What focus?" }).fill("Security");
  await dialog.getByRole("button", { name: "Use prompt" }).click();
  await expect(dialog).toHaveCount(0);
  await expect(box(page)).toHaveValue("Review for Security.");
  // still nothing sent: the text is the person's to send
  expect(bodies).toHaveLength(1);
});

test("a command that takes no arguments is inserted alone, and the list closes", async ({
  page,
  session,
}) => {
  await session.lists("adam", COMMANDS);
  await page.goto("/");
  await box(page).fill("/explain");
  await expect(commandRows(page)).toHaveCount(1);
  // Tab picks, as Enter does
  await page.keyboard.press("Tab");
  await expect(box(page)).toHaveValue("/explain");
  await expect(menu(page)).toHaveCount(0);
  await expect(page).not.toHaveURL(THREAD_URL);
});

test("an agent that serves no command shows the prompts group alone", async ({ page, session }) => {
  await session.prompt(reviewPrompt);
  await page.goto("/");
  await expect(box(page)).toBeVisible();
  await box(page).fill("/");
  await expect(menu(page)).toBeVisible();
  await expect(promptsGroup(page)).toBeVisible();
  await expect(commandsGroup(page)).toHaveCount(0);
  await expect(promptRows(page)).toHaveCount(1);
  // a word that starts no command is the prompts' as it always was
  await box(page).fill("/rev");
  await expect(promptRows(page)).toHaveCount(1);
  await expect(commandsGroup(page)).toHaveCount(0);
});

test("a query neither half matches keeps the one menu open, and sends the text", async ({
  page,
  session,
}) => {
  await session.lists("adam", COMMANDS);
  await session.prompt(reviewPrompt);
  const bodies = posted(page);
  await page.goto("/");
  await box(page).fill("/zzz");
  await expect(menu(page)).toBeVisible();
  // no row at all: no group, no listbox — the prompts' name and the line are the menu
  await expect(commandsGroup(page)).toHaveCount(0);
  await expect(promptsGroup(page)).toHaveCount(0);
  await expect(page.getByRole("listbox")).toHaveCount(0);
  await expect(page.getByText("Your prompts", { exact: true })).toBeVisible();
  await expect(page.getByText(/No prompt matches/)).toBeVisible();
  await page.keyboard.press("Enter");
  await expect(page).toHaveURL(THREAD_URL);
  expect(bodies[0]?.messages[0]?.content).toBe("/zzz");
});

for (const scheme of ["light", "dark"] as const) {
  test.describe(`accessibility (${scheme})`, () => {
    test.use({ colorScheme: scheme, contextOptions: { reducedMotion: "reduce" } });

    test("axe: the menu with both groups, and the box after a pick, have no serious violations", async ({
      page,
      session,
    }) => {
      await session.lists("adam", COMMANDS);
      await session.prompt(reviewPrompt);
      await page.goto("/");
      await expect(box(page)).toBeVisible();
      expect(await axeViolations(page)).toEqual([]);
      await box(page).fill("/");
      await expect(commandsGroup(page)).toBeVisible();
      await expect(promptsGroup(page)).toBeVisible();
      expect(await axeViolations(page)).toEqual([]);
      // the active row crosses into the prompts group and stays legible there
      await page.keyboard.press("ArrowDown");
      await page.keyboard.press("ArrowDown");
      await expect(promptRows(page).nth(0)).toHaveAttribute("aria-selected", "true");
      expect(await axeViolations(page)).toEqual([]);
      await page.keyboard.press("ArrowUp");
      await page.keyboard.press("ArrowUp");
      await page.keyboard.press("Enter");
      await expect(box(page)).toHaveValue("/explain");
      expect(await axeViolations(page)).toEqual([]);
      await expectNoHorizontalScroll(page);
    });
  });
}
