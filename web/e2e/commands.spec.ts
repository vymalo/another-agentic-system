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
 * The `/` menu of an agent's own commands (ADR 0079, docs/api/commands-v1.md section 6, web/README.md
 * "The agent's own commands"), against the mock: the list comes from the agent's card as the
 * capabilities document relays it, a pick writes `/name ` into the box and nothing else, the message
 * that goes out is that text, and an agent that serves no command keeps the prompts menu of today.
 * The mock lists no command for any agent until a test says so for its own session.
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
};

/** The mock keeps the cards per session, named by a cookie: a test is its own. */
const test = base.extend<{ session: Session }>({
  session: async ({ context, request }, use, info) => {
    const name = `e2e-commands-${info.workerIndex}-${info.testId}-${info.repeatEachIndex}`;
    await context.addCookies([{ name: "mock-registry", value: name, url: ORIGIN }]);
    const hook = async (path: string, data?: unknown) => {
      const res = await request.post(
        `${MOCK_URL}/__mock/${path}${path.includes("?") ? "&" : "?"}session=${name}`,
        data === undefined ? {} : { data },
      );
      expect(res.ok()).toBe(true);
    };
    await use({ lists: (agent, params) => hook("commands", { agent, params }) });
  },
});

const box = (page: Page) => page.getByLabel("Message", { exact: true });
/** The agent's commands menu, and the person's prompts menu: one of them, never both. */
const commandsMenu = (page: Page) => page.locator('[data-slot="command-menu"]');
const promptsMenu = (page: Page) => page.locator('[data-slot="prompt-menu"]');
/** The rows of the commands menu. */
const rows = (page: Page) =>
  page.getByRole("listbox", { name: "Agent commands" }).getByRole("option");

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

test("the menu lists the agent's commands, and a pick inserts /name and sends it as the message", async ({
  page,
  session,
}) => {
  await session.lists("adam", COMMANDS);
  const bodies = posted(page);
  await page.goto("/");
  await expect(box(page)).toBeVisible();

  // "/" alone is the agent's commands: sorted by name, each with its description and its hint
  await box(page).fill("/");
  await expect(commandsMenu(page)).toBeVisible();
  await expect(page.getByText("Agent commands", { exact: true })).toBeVisible();
  await expect(rows(page)).toHaveCount(2);
  await expect(rows(page).nth(0)).toContainText("/explain");
  await expect(rows(page).nth(0)).toContainText("Explains a file.");
  await expect(rows(page).nth(1)).toContainText("/review");
  await expect(rows(page).nth(1)).toContainText("<repo>");
  await expect(rows(page).nth(1)).toContainText("Reviews a change.");
  // the focus stays in the box, which names the active row
  await expect(box(page)).toBeFocused();
  await expect(box(page)).toHaveAttribute(
    "aria-activedescendant",
    (await rows(page).nth(0).getAttribute("id")) ?? "",
  );
  // the prompts list is not there, and the person's library was not read for this word
  await expect(promptsMenu(page)).toHaveCount(0);

  await box(page).fill("/rev");
  await expect(rows(page)).toHaveCount(1);
  await page.keyboard.press("Enter");
  // the text, and the caret after the space for the arguments: nothing was sent
  await expect(box(page)).toHaveValue("/review ");
  await expect(commandsMenu(page)).toHaveCount(0);
  expect(bodies).toHaveLength(0);

  await page.keyboard.type("acme/widgets");
  await expect(box(page)).toHaveValue("/review acme/widgets");
  await page.getByRole("button", { name: "Send" }).click();
  await expect(page).toHaveURL(THREAD_URL);
  // the message is the text, and the mock echoes what arrived
  expect(bodies[0]?.messages[0]?.content).toBe("/review acme/widgets");
  await expect(conversation(page).getByText("echo: /review acme/widgets")).toBeVisible();
  await expectNoHorizontalScroll(page);
});

test("a command that takes no arguments is inserted alone, and the list closes", async ({
  page,
  session,
}) => {
  await session.lists("adam", COMMANDS);
  await page.goto("/");
  await box(page).fill("/explain");
  await expect(rows(page)).toHaveCount(1);
  // Tab picks, as Enter does
  await page.keyboard.press("Tab");
  await expect(box(page)).toHaveValue("/explain");
  await expect(commandsMenu(page)).toHaveCount(0);
  await expect(page).not.toHaveURL(THREAD_URL);
});

test("an agent that serves no command keeps the prompts menu, for / and for any word", async ({
  page,
}) => {
  await page.goto("/");
  await expect(box(page)).toBeVisible();
  await box(page).fill("/");
  await expect(promptsMenu(page)).toBeVisible();
  await expect(page.getByText("Your prompts", { exact: true })).toBeVisible();
  await expect(page.getByText("You have no prompts yet.")).toBeVisible();
  await expect(commandsMenu(page)).toHaveCount(0);
  // a word that starts no command is the prompts' as it always was (the library here is empty, so
  // it says that rather than "No prompt matches")
  await box(page).fill("/zzz");
  await expect(promptsMenu(page)).toBeVisible();
  await expect(page.getByText("You have no prompts yet.")).toBeVisible();
  await expect(commandsMenu(page)).toHaveCount(0);
});

test("a query that starts no command of an agent that serves some opens the prompts", async ({
  page,
  session,
}) => {
  await session.lists("adam", COMMANDS);
  await page.goto("/");
  await expect(box(page)).toBeVisible();
  await box(page).fill("/zzz");
  await expect(promptsMenu(page)).toBeVisible();
  await expect(commandsMenu(page)).toHaveCount(0);
  // and a word that starts one is the agent's, even though a prompt may hold it too
  await box(page).fill("/exp");
  await expect(commandsMenu(page)).toBeVisible();
  await expect(promptsMenu(page)).toHaveCount(0);
});

for (const scheme of ["light", "dark"] as const) {
  test.describe(`accessibility (${scheme})`, () => {
    test.use({ colorScheme: scheme, contextOptions: { reducedMotion: "reduce" } });

    test("axe: the commands menu, and the box after a pick, have no serious violations", async ({
      page,
      session,
    }) => {
      await session.lists("adam", COMMANDS);
      await page.goto("/");
      await expect(box(page)).toBeVisible();
      expect(await axeViolations(page)).toEqual([]);
      await box(page).fill("/");
      await expect(commandsMenu(page)).toBeVisible();
      expect(await axeViolations(page)).toEqual([]);
      await page.keyboard.press("Enter");
      await expect(box(page)).toHaveValue("/explain");
      expect(await axeViolations(page)).toEqual([]);
      await expectNoHorizontalScroll(page);
    });
  });
}
