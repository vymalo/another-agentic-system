import AxeBuilder from "@axe-core/playwright";
import { test as base, expect, type Page } from "@playwright/test";
import { animationsDone, expectNoHorizontalScroll, MOCK_URL } from "./helpers";

/*
 * The `/` menu of the message box and the form that fills a prompt (docs/api/prompts-v1.md, the
 * recipe), against the mock's prompt library: the list opens on a message that is one `/name`, a
 * pick reads the prompt, the form draws its variables, and `renderPrompt`'s text goes into the box
 * for the person to edit. Nothing is sent, and a prompt's words are plain text.
 */

const ORIGIN = "http://127.0.0.1:3000";

type Mine = {
  call: (method: string, path: string, body?: unknown) => Promise<Response>;
  seed: (body: unknown) => Promise<{ id: string; revision: string; name: string }>;
  share: (prompts: unknown[]) => Promise<void>;
  as: (profile: string) => Promise<void>;
};

const test = base.extend<{ mine: Mine }>({
  mine: async ({ context }, use, info) => {
    const session = `prompt-composer-${info.project.name}-${info.workerIndex}-${info.testId}`;
    await context.addCookies([{ name: "mock-registry", value: session, url: ORIGIN }]);
    const hook = async (path: string, body?: unknown) => {
      const res = await fetch(
        `${MOCK_URL}${path}${path.includes("?") ? "&" : "?"}session=${session}`,
        {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          ...(body === undefined ? {} : { body: JSON.stringify(body) }),
        },
      );
      expect(res.ok, await res.clone().text()).toBe(true);
    };
    const call: Mine["call"] = (method, path, body) =>
      fetch(`${MOCK_URL}${path}`, {
        method,
        headers: { "Content-Type": "application/json", Cookie: `mock-registry=${session}` },
        ...(body === undefined ? {} : { body: JSON.stringify(body) }),
      });
    await use({
      call,
      seed: async (body) => {
        const res = await call("POST", "/api/prompts", body);
        expect(res.status, await res.clone().text()).toBe(201);
        return (await res.json()) as { id: string; revision: string; name: string };
      },
      share: (prompts) => hook("/__mock/prompts/shared", prompts),
      as: (profile) => hook(`/__mock/config?me=${profile}`),
    });
  },
});

const review = (over: Record<string, unknown> = {}) => ({
  name: "review",
  title: "Review a change",
  description: "Reviews a change for one concern",
  body: "Review for {{focus}} at depth {{depth}}.",
  variables: [
    {
      id: "focus",
      question: "What focus?",
      options: [
        { value: "security", label: "Security" },
        { value: "speed", label: "Speed" },
      ],
    },
    { id: "depth", question: "How deep?", type: "number", unit: "levels" },
  ],
  ...over,
});

const composer = (page: Page) => page.getByLabel("Message", { exact: true });
const menu = (page: Page) => page.locator('[data-slot="slash-menu"]');
const dialog = (page: Page) => page.getByRole("dialog");

async function open(page: Page) {
  await page.goto("/");
  await expect(composer(page)).toBeVisible();
  await expect(page.getByRole("button", { name: "Send" })).toBeVisible();
}

test("the / menu lists the person's prompts; a pick fills the form and puts the text in the box, never sending it", async ({
  page,
  mine,
}) => {
  const made = await mine.seed(review());
  await mine.seed({ name: "notes", title: "Write notes", body: "Write the notes." });
  await mine.share([
    {
      name: "release-notes",
      title: "Release notes",
      description: "Turns merged pull requests into release notes.",
      body: "Write release notes for {{version}}.",
      variables: [{ id: "version", question: "Which version?", type: "text" }],
      audience: ["*"],
    },
  ]);
  const renders: unknown[] = [];
  const sends: string[] = [];
  page.on("request", (request) => {
    const path = new URL(request.url()).pathname;
    if (request.method() === "POST" && path.endsWith("/render")) {
      renders.push(request.postDataJSON());
    }
    if (request.method() === "POST" && path.startsWith("/agui/")) sends.push(path);
  });

  const config = page.waitForResponse("**/api/config");
  await open(page);
  await config;
  // a slash in the middle of a message is a message, not a command
  await composer(page).fill("ask /rev");
  await expect(menu(page)).toHaveCount(0);

  // the full list: the arrows move the active row, and the box keeps the focus and names it
  await composer(page).fill("/");
  const all = menu(page).getByRole("option");
  await expect(all).toHaveCount(3);
  await expect(all.nth(0)).toHaveAttribute("aria-selected", "true");
  await page.keyboard.press("ArrowDown");
  await expect(all.nth(1)).toHaveAttribute("aria-selected", "true");
  await expect(composer(page)).toHaveAttribute(
    "aria-activedescendant",
    (await all.nth(1).getAttribute("id")) ?? "",
  );
  await page.keyboard.press("ArrowUp");
  await expect(all.nth(0)).toHaveAttribute("aria-selected", "true");

  await composer(page).fill("/rev");
  await expect(menu(page)).toBeVisible();
  await expect(menu(page).getByRole("option")).toHaveCount(1);
  await expect(menu(page).getByRole("option")).toContainText("/review");
  await expect(menu(page).getByRole("option")).toContainText("Review a change");
  await expect(menu(page).getByRole("option")).toContainText("Reviews a change for one concern");

  const option = menu(page).getByRole("option");
  await expect(option).toHaveAttribute("aria-selected", "true");
  await expect(composer(page)).toHaveAttribute(
    "aria-activedescendant",
    (await option.getAttribute("id")) ?? "",
  );
  // Tab picks, as Enter does, and the focus stays in the box for the form
  await page.keyboard.press("Tab");

  await expect(dialog(page)).toBeVisible();
  await expect(dialog(page).getByRole("heading", { name: "Review a change" })).toBeVisible();
  await expect(composer(page)).toHaveValue("");
  // the button says what is missing, and holds until both questions are answered
  await expect(dialog(page).getByRole("button", { name: "Use prompt" })).toHaveAttribute(
    "aria-disabled",
    "true",
  );
  await expect(dialog(page)).toContainText("2 questions are not answered yet.");
  await dialog(page).getByText("Security", { exact: true }).click();
  await dialog(page)
    .getByRole("textbox", { name: /How deep\?/ })
    .fill("3");
  await expect(dialog(page).getByRole("button", { name: "Use prompt" })).not.toHaveAttribute(
    "aria-disabled",
    "true",
  );
  await dialog(page).getByRole("button", { name: "Use prompt" }).click();

  await expect(dialog(page)).toHaveCount(0);
  await expect(composer(page)).toHaveValue("Review for Security at depth 3.");
  expect(renders).toEqual([
    {
      revision: made.revision,
      answers: [
        { id: "focus", values: ["security"] },
        { id: "depth", values: ["3"] },
      ],
    },
  ]);
  // the text is the person's to send: nothing did
  expect(sends).toEqual([]);

  // a prompt shared with the person carries a badge and names no author
  await composer(page).fill("/release");
  await expect(menu(page)).toBeVisible();
  await expect(menu(page).getByRole("option")).toContainText("Shared");
  await expect(menu(page)).not.toContainText(/@|owner|author/i);
  await expectNoHorizontalScroll(page);
});

test("a prompt with no questions is filled at once with answers: [], and Escape closes the list", async ({
  page,
  mine,
}) => {
  const notes = await mine.seed({ name: "notes", title: "Write notes", body: "Write the notes." });
  const renders: unknown[] = [];
  page.on("request", (request) => {
    if (request.method() === "POST" && request.url().endsWith("/render")) {
      renders.push(request.postDataJSON());
    }
  });
  const config = page.waitForResponse("**/api/config");
  await open(page);
  await config;
  await composer(page).fill("/notes");
  await expect(menu(page)).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(menu(page)).toHaveCount(0);
  await expect(composer(page)).toHaveValue("/notes");
  // a changed query opens it again; Escape closed the list, not the command
  await composer(page).fill("/note");
  await expect(menu(page).getByRole("option")).toContainText("/notes");
  await page.keyboard.press("Enter");
  await expect(composer(page)).toHaveValue("Write the notes.");
  await expect(dialog(page)).toHaveCount(0);
  expect(renders).toEqual([{ revision: notes.revision, answers: [] }]);
});

test("a stale prompt is never overwritten: the form offers the current version and keeps the answers that fit", async ({
  page,
  mine,
}) => {
  const made = await mine.seed(review());
  const config = page.waitForResponse("**/api/config");
  await open(page);
  await config;
  await composer(page).fill("/rev");
  await expect(menu(page).getByRole("option")).toContainText("/review");
  await page.keyboard.press("Enter");
  await expect(dialog(page)).toBeVisible();
  await dialog(page).getByText("Security", { exact: true }).click();
  await dialog(page)
    .getByRole("textbox", { name: /How deep\?/ })
    .fill("3");

  // another tab changes the prompt: the revision the form holds is stale
  const other = await mine.call("PATCH", `/api/prompts/${made.id}`, {
    revision: made.revision,
    title: "Review a change, again",
    body: "Review for {{focus}} at depth {{depth}} on {{day}}.",
    variables: [...review().variables, { id: "day", question: "Which day?", type: "text" }],
  });
  expect(other.status).toBe(200);
  await dialog(page).getByRole("button", { name: "Use prompt" }).click();
  await expect(dialog(page).getByRole("alert")).toContainText("changed elsewhere");
  await expect(dialog(page).getByRole("alert")).toContainText("Your answers are still here");
  // the answers are still in the form, and nothing was overwritten
  await expect(dialog(page).getByRole("radio", { name: "Security" })).toBeChecked();
  await expect(dialog(page).getByRole("textbox", { name: /How deep\?/ })).toHaveValue("3");

  await dialog(page).getByRole("button", { name: "Load the current version" }).click();
  await expect(dialog(page).getByRole("heading", { name: "Review a change, again" })).toBeVisible();
  await expect(dialog(page).getByRole("radio", { name: "Security" })).toBeChecked();
  await expect(dialog(page).getByRole("textbox", { name: /How deep\?/ })).toHaveValue("3");
  await dialog(page)
    .getByRole("textbox", { name: /Which day\?/ })
    .fill("Friday");
  await dialog(page).getByRole("button", { name: "Use prompt" }).click();
  await expect(composer(page)).toHaveValue("Review for Security at depth 3 on Friday.");
});

test("without a library in the configuration nothing opens, and no prompt is asked for", async ({
  page,
}) => {
  const asked: string[] = [];
  page.on("request", (request) => {
    if (new URL(request.url()).pathname.startsWith("/api/prompts")) asked.push(request.url());
  });
  await page.route("**/api/config", async (route) => {
    const res = await route.fetch();
    const body = (await res.json()) as Record<string, unknown>;
    delete body.prompts;
    await route.fulfill({ response: res, json: body });
  });
  // the configuration was answered: the absence of the menu is not the absence of an answer
  const config = page.waitForResponse("**/api/config");
  await open(page);
  await config;
  await composer(page).fill("/");
  await expect(menu(page)).toHaveCount(0);
  expect(asked).toEqual([]);
});

for (const scheme of ["light", "dark"] as const) {
  test.describe(`accessibility (${scheme})`, () => {
    test.use({ colorScheme: scheme, contextOptions: { reducedMotion: "reduce" } });

    async function serious(page: Page) {
      await animationsDone(page);
      const results = await new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa"]).analyze();
      return results.violations.filter((v) => v.impact === "serious" || v.impact === "critical");
    }

    test("axe: the open menu and the fill form have no serious violations", async ({
      page,
      mine,
    }) => {
      await mine.seed(review());
      const config = page.waitForResponse("**/api/config");
      await open(page);
      await config;
      await composer(page).fill("/rev");
      await expect(menu(page)).toBeVisible();
      expect(await serious(page)).toEqual([]);

      // the list with no match, whose line is a sibling of the listbox
      await composer(page).fill("/zzz");
      await expect(page.getByText(/No prompt matches/)).toBeVisible();
      expect(await serious(page)).toEqual([]);

      await composer(page).fill("/rev");
      await expect(menu(page).getByRole("option")).toContainText("/review");
      await page.keyboard.press("Enter");
      await expect(dialog(page)).toBeVisible();
      expect(await serious(page)).toEqual([]);
      await expectNoHorizontalScroll(page);
    });
  });
}
