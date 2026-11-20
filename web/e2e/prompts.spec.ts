import AxeBuilder from "@axe-core/playwright";
import { test as base, expect, type Locator, type Page } from "@playwright/test";
import { animationsDone, errorLine, expectNoHorizontalScroll, MOCK_URL } from "./helpers";

/*
 * "My prompts" (ADR 0070, web/DESIGN.md "My prompts"), against the mock's prompt library, which holds the contract's own rules
 * (docs/api/prompts-v1.md): the page, the form with its seven kinds of question, what is kept and not shown, the refusals at
 * their fields, a stale edit, delete, a shared prompt read only and copied. The mock keeps a person's prompts per session,
 * named by a cookie, so each test has its own and nobody else's. A test reaches the same routes the page does (with the
 * session's cookie) to set up what it needs and to change a prompt "in another tab".
 */

const ORIGIN = "http://127.0.0.1:3000";

type Mine = {
  /** A call to the mock's own routes with this session's cookie: what "another tab" does. */
  call: (method: string, path: string, body?: unknown) => Promise<Response>;
  /** A prompt of the person's own, made behind the page's back. */
  seed: (body: unknown) => Promise<{ id: string; revision: string; name: string }>;
  /** Prompts "of somebody else" shared with everyone, for this session. */
  share: (prompts: unknown[]) => Promise<void>;
  as: (profile: string) => Promise<void>;
};

const test = base.extend<{ mine: Mine }>({
  mine: async ({ context }, use, info) => {
    const session = `prompts-${info.project.name}-${info.workerIndex}-${info.testId}`;
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

const FOCUS_CHOICES = [
  { value: "security", label: "Security" },
  { value: "speed", label: "Speed" },
];

const review = (over: Record<string, unknown> = {}) => ({
  name: "review",
  title: "Review a change",
  description: "Reviews a change for one concern",
  body: "Review for {{focus}} at depth {{depth}}.",
  variables: [
    { id: "focus", question: "What focus?", options: FOCUS_CHOICES },
    { id: "depth", question: "How deep?", type: "number", unit: "levels" },
  ],
  ...over,
});

const rowOf = (page: Page, title: string): Locator =>
  page
    .locator('[data-slot="prompt-row"]')
    .filter({ has: page.getByRole("heading", { name: title, exact: true }) });

/** What the focus is on, as a selection of the text box when it is one. */
const selection = (page: Page) =>
  page.evaluate(() => {
    const el = document.activeElement as HTMLTextAreaElement | null;
    return {
      tag: el?.tagName ?? "",
      text: el && "value" in el ? el.value.slice(el.selectionStart ?? 0, el.selectionEnd ?? 0) : "",
    };
  });

test("a prompt is created with two questions and listed by name; its words are in one request body and nowhere else", async ({
  page,
  mine,
}) => {
  const SENTINEL = "zq-sentinel-wording-4417";
  const sentIn: string[] = [];
  const addresses: string[] = [];
  page.on("request", (request) => {
    addresses.push(request.url());
    if ((request.postData() ?? "").includes(SENTINEL)) {
      sentIn.push(`${request.method()} ${new URL(request.url()).pathname}`);
    }
  });
  await page.goto("/prompts");
  await expect(page.getByRole("heading", { level: 1, name: "My prompts" })).toBeVisible();
  await expect(page.getByText("You have no prompts yet.")).toBeVisible();

  await page.getByRole("button", { name: "New prompt" }).click();
  await expect(page.getByRole("heading", { name: "New prompt" })).toBeFocused();
  await page.getByLabel("Title").fill(`Review ${SENTINEL}`);
  // the name follows the title until the person writes one of their own
  await expect(page.getByLabel("Name", { exact: true })).toHaveValue(
    "review-zq-sentinel-wording-4417",
  );
  await page.getByLabel("Name", { exact: true }).fill("review");
  await page.getByLabel("Description (optional)").fill("Reviews a change");
  await page.getByLabel("Text", { exact: true }).fill("Review for {{focus}} at depth {{depth}}.");

  await page.getByRole("button", { name: "Add a question" }).click();
  await page.getByLabel("Question", { exact: true }).fill("What focus?");
  // the name in the text is made from the first words
  await expect(page.getByLabel("Name in the text")).toHaveValue("what-focus");
  await page.getByLabel("Name in the text").fill("focus");
  await page.getByLabel("Option 1 of question 1", { exact: true }).fill("Security");
  await page.getByLabel("Option 2 of question 1", { exact: true }).fill("Speed");

  await page.getByRole("button", { name: "Add a question" }).click();
  const second = page.locator('[data-slot="question-editor"]').nth(1);
  await second.getByLabel("Question", { exact: true }).fill("How deep?");
  await second.getByLabel("Name in the text").fill("depth");
  await second.getByLabel("Kind").selectOption("number");
  await second.getByLabel("Unit (optional)").fill("levels");
  await page.getByRole("button", { name: "Save prompt" }).click();

  await expect(page.getByText(`Saved “Review ${SENTINEL}”.`)).toBeVisible();
  const row = rowOf(page, `Review ${SENTINEL}`);
  await expect(row).toContainText("/review");
  await expect(row).toContainText("Asks 2 questions");
  await expect(page.getByText("(1 of 100)")).toBeVisible();
  await expectNoHorizontalScroll(page);

  // what the server holds is what was typed, the choices' values made from their labels
  const stored = (await (await mine.call("GET", "/api/prompts")).json()) as {
    items: { id: string }[];
  };
  const full = (await (await mine.call("GET", `/api/prompts/${stored.items[0]?.id}`)).json()) as {
    variables: unknown[];
  };
  expect(full.variables).toEqual([
    {
      id: "focus",
      question: "What focus?",
      type: "single",
      options: [
        { value: "security", label: "Security" },
        { value: "speed", label: "Speed" },
      ],
    },
    { id: "depth", question: "How deep?", type: "number", unit: "levels" },
  ]);

  // the words travel in one request body: not in an address, not in what the browser stores
  expect(sentIn).toEqual(["POST /api/prompts"]);
  expect(addresses.filter((a) => a.includes(SENTINEL))).toEqual([]);
  const stores = await page.evaluate(() => [
    ...Object.entries(localStorage),
    ...Object.entries(sessionStorage),
    document.cookie,
  ]);
  expect(JSON.stringify(stores)).not.toContain(SENTINEL);
  expect(page.url()).not.toContain(SENTINEL);
});

test("the list holds the person's own and the ones shared with them; a shared one opens read only and names no author", async ({
  page,
  mine,
}) => {
  await mine.seed(review());
  await mine.share([
    {
      name: "deploy-notes",
      title: "Deploy notes",
      description: "Notes for a deploy",
      body: "Write the <b>notes</b> for the deploy.",
      audience: ["*"],
    },
  ]);
  await page.goto("/prompts");
  const own = page.getByRole("list", { name: "Your prompts" });
  await expect(own.getByRole("heading", { name: "Review a change" })).toBeVisible();
  await expect(own.getByText("/review", { exact: true })).toBeVisible();
  const shared = page.getByRole("list", { name: "Shared with you" });
  const row = rowOf(page, "Deploy notes");
  await expect(shared).toContainText("Deploy notes");
  await expect(row.getByText("Shared with you")).toBeVisible();
  // nothing to change, and nobody named
  await expect(row.getByRole("button", { name: /^(Edit|Delete)/ })).toHaveCount(0);
  await expect(
    row.getByRole("button", { name: /^(Share|Change sharing|Stop sharing)/ }),
  ).toHaveCount(0);
  await expect(row).not.toContainText(/@|owner|author/i);

  await row.getByRole("button", { name: "View “Deploy notes”" }).click();
  await expect(page.getByRole("heading", { name: "Deploy notes", level: 2 })).toBeFocused();
  // its words are plain text: the tag is characters on the page, not an element
  await expect(page.getByText("Write the <b>notes</b> for the deploy.")).toBeVisible();
  await expect(page.locator('[data-slot="shared-prompt-body"] b')).toHaveCount(0);
  await expect(page.getByRole("textbox")).toHaveCount(0);
  await expect(page.locator("main")).not.toContainText(/@|owner|author/i);
  await expectNoHorizontalScroll(page);
  await page.getByRole("button", { name: "Back to your prompts" }).click();
  await expect(page.getByRole("button", { name: "View “Deploy notes”" })).toBeFocused();
});

test("a shared prompt is copied into the person's own, and a name they already use is refused at the name", async ({
  page,
  mine,
}) => {
  await mine.share([
    { name: "plan", title: "Their plan", body: "Plan the work.", audience: ["*"] },
    { name: "review", title: "Their review", body: "Review it.", audience: ["*"] },
  ]);
  await mine.seed({ name: "review", title: "My review", body: "Mine." });
  await page.goto("/prompts");

  await rowOf(page, "Their plan")
    .getByRole("button", { name: "Copy to my prompts: “Their plan”" })
    .click();
  await expect(page.getByRole("heading", { name: "Copy to my prompts" })).toBeFocused();
  await expect(page.getByLabel("Title")).toHaveValue("Their plan");
  await expect(page.getByLabel("Name", { exact: true })).toHaveValue("plan");
  await expect(page.getByLabel("Text", { exact: true })).toHaveValue("Plan the work.");
  await page.getByRole("button", { name: "Save prompt" }).click();
  await expect(page.getByText("Saved “Their plan”.")).toBeVisible();
  const mineNow = page.getByRole("list", { name: "Your prompts" });
  await expect(mineNow.getByText("/plan", { exact: true })).toBeVisible();
  // a copy is private
  const copied = (await (await mine.call("GET", "/api/prompts?scope=mine")).json()) as {
    items: { name: string; audience?: string[] }[];
  };
  expect(copied.items.find((p) => p.name === "plan")?.audience ?? []).toEqual([]);

  // the name `review` is already the person's own
  await rowOf(page, "Their review")
    .getByRole("button", { name: "Copy to my prompts: “Their review”" })
    .click();
  await page.getByRole("button", { name: "Save prompt" }).click();
  const name = page.getByLabel("Name", { exact: true });
  await expect(name).toBeFocused();
  await expect(name).toHaveAttribute("aria-invalid", "true");
  await expect(
    page.getByText("You already have a prompt named /review. Choose another name."),
  ).toBeVisible();
  // the person's own text is still in the form, and a new name saves it
  await expect(page.getByLabel("Title")).toHaveValue("Their review");
  await name.fill("their-review");
  await page.getByRole("button", { name: "Save prompt" }).click();
  await expect(page.getByText("Saved “Their review”.")).toBeVisible();
});

test("an edit renames a prompt and changes its text; only what changed is sent, with the revision", async ({
  page,
  mine,
}) => {
  const made = await mine.seed(review());
  const patches: { revision: string; [k: string]: unknown }[] = [];
  page.on("request", (request) => {
    if (request.method() === "PATCH") patches.push(request.postDataJSON());
  });
  await page.goto("/prompts");
  await rowOf(page, "Review a change")
    .getByRole("button", { name: "Edit “Review a change”" })
    .click();
  await expect(page.getByRole("heading", { name: "Edit “Review a change”" })).toBeFocused();
  await page.getByLabel("Name", { exact: true }).fill("review-it");
  await page
    .getByLabel("Text", { exact: true })
    .fill("Review for {{focus}} at depth {{depth}} now.");
  await page.getByRole("button", { name: "Save changes" }).click();
  await expect(page.getByText("Saved “Review a change”.")).toBeVisible();

  expect(patches).toHaveLength(1);
  // the members that changed, the revision it was read at, and nothing else: no variables, no audience
  expect(Object.keys(patches[0] ?? {}).sort()).toEqual(["body", "name", "revision"]);
  expect(patches[0]?.revision).toBe(made.revision);
  const after = (await (await mine.call("GET", `/api/prompts/${made.id}`)).json()) as {
    name: string;
    body: string;
    title: string;
  };
  expect(after).toMatchObject({
    name: "review-it",
    body: "Review for {{focus}} at depth {{depth}} now.",
    title: "Review a change",
  });

  // nothing changed: no request, and the form closes
  await rowOf(page, "Review a change")
    .getByRole("button", { name: "Edit “Review a change”" })
    .click();
  await page.getByRole("button", { name: "Save changes" }).click();
  await expect(page.getByRole("button", { name: "New prompt" })).toBeVisible();
  expect(patches).toHaveLength(1);
});

test("a stale edit keeps the person's text, says so, and Load the current version starts again from it", async ({
  page,
  mine,
}) => {
  const made = await mine.seed(review());
  await page.goto("/prompts");
  await rowOf(page, "Review a change")
    .getByRole("button", { name: "Edit “Review a change”" })
    .click();
  await expect(page.getByLabel("Title")).toHaveValue("Review a change");
  // another tab renames it: the revision the form holds is now stale
  const other = await mine.call("PATCH", `/api/prompts/${made.id}`, {
    revision: made.revision,
    title: "Renamed elsewhere",
  });
  expect(other.status).toBe(200);

  await page.getByLabel("Title").fill("My own new title");
  await page.getByRole("button", { name: "Save changes" }).click();
  await expect(errorLine(page)).toContainText("changed elsewhere since you opened it");
  await expect(errorLine(page)).toContainText("Your changes are still here");
  await expect(page.getByLabel("Title")).toHaveValue("My own new title");
  // again: still stale, never an overwrite
  await page.getByRole("button", { name: "Save changes" }).click();
  await expect(errorLine(page)).toContainText("Your changes are still here");
  const held = (await (await mine.call("GET", `/api/prompts/${made.id}`)).json()) as {
    title: string;
  };
  expect(held.title).toBe("Renamed elsewhere");

  await page.getByRole("button", { name: "Load the current version" }).click();
  await expect(page.getByLabel("Title")).toHaveValue("Renamed elsewhere");
  await expect(page.getByRole("button", { name: "Load the current version" })).toHaveCount(0);
  await page.getByLabel("Title").fill("Final title");
  await page.getByRole("button", { name: "Save changes" }).click();
  await expect(page.getByText("Saved “Final title”.")).toBeVisible();
});

test("a refused save puts the words at the field `at` names and moves the focus there", async ({
  page,
  mine,
}) => {
  await mine.as("user");
  await page.goto("/prompts");
  await page.getByRole("button", { name: "New prompt" }).click();
  await page.getByLabel("Title").fill("Refusals");
  await page.getByLabel("Name", { exact: true }).fill("Bad Name");
  await page.getByLabel("Text", { exact: true }).fill("Hello");
  await page.getByRole("button", { name: "Save prompt" }).click();

  // bad_name: the name field, marked and described
  const name = page.getByLabel("Name", { exact: true });
  await expect(name).toBeFocused();
  await expect(name).toHaveAttribute("aria-invalid", "true");
  await expect(
    page.getByRole("alert").filter({ hasText: "1 to 32 lowercase letters" }),
  ).toBeVisible();
  const described = await name.getAttribute("aria-describedby");
  expect(described?.split(" ").length).toBeGreaterThan(1);
  await expect(page.getByLabel("Title")).not.toHaveAttribute("aria-invalid", "true");

  // bad_reference: the text, with the two braces that start the blank selected
  await name.fill("refusals");
  await page.getByLabel("Text", { exact: true }).fill("Hello {{ x }} and more");
  await page.getByRole("button", { name: "Save prompt" }).click();
  await expect(page.getByLabel("Text", { exact: true })).toBeFocused();
  await expect(page.getByText(/These braces do not start a blank/)).toBeVisible();
  expect(await selection(page)).toEqual({ tag: "TEXTAREA", text: "{{" });

  // undeclared_variable: the blank is selected and named
  await page.getByLabel("Text", { exact: true }).fill("Hello {{who}} and more");
  await page.getByRole("button", { name: "Save prompt" }).click();
  await expect(
    page.getByText("{{who}} is not one of the questions. Add a question named who"),
  ).toBeVisible();
  expect(await selection(page)).toEqual({ tag: "TEXTAREA", text: "{{who}}" });

  // unused_variable: on the question, whose card takes the focus
  await page.getByLabel("Text", { exact: true }).fill("Hello");
  await page.getByRole("button", { name: "Add a question" }).click();
  await page.getByLabel("Question", { exact: true }).fill("Who?");
  await page.getByLabel("Kind").selectOption("text");
  await page.getByRole("button", { name: "Save prompt" }).click();
  const card = page.getByRole("heading", { name: "Question 1" });
  await expect(card).toBeFocused();
  await expect(page.locator('[data-slot="question-editor"]')).toContainText(
    "This question is not used in the text. Write {{who}} where its answer goes",
  );
  // the answer to it: write the name in the text, and the prompt is saved
  await page.getByRole("button", { name: "Insert {{who}} into the text" }).click();
  await page.getByRole("button", { name: "Save prompt" }).click();
  await expect(page.getByText("Saved “Refusals”.")).toBeVisible();
});

test("a scale and a rank, which the editor cannot edit, are shown as kept and survive a save of another field byte for byte", async ({
  page,
  mine,
}) => {
  const made = await mine.seed({
    name: "mixed",
    title: "Mixed",
    body: "Do {{what}} at {{depth}} by {{order}}.",
    variables: [
      {
        id: "what",
        question: "What?",
        options: [
          { value: "a", label: "A", description: "The first", icon: "git", recommended: true },
          { value: "b", label: "B" },
        ],
        default: "a",
      },
      {
        id: "depth",
        question: "How deep?",
        type: "scale",
        to: 5,
        fromLabel: "Shallow",
        toLabel: "Deep",
      },
      {
        id: "order",
        question: "In which order?",
        type: "rank",
        options: [
          { value: "x", label: "X" },
          { value: "y", label: "Y" },
        ],
        default: ["y", "x"],
      },
    ],
  });
  const before = await (await mine.call("GET", `/api/prompts/${made.id}`)).json();
  await page.goto("/prompts");
  await rowOf(page, "Mixed").getByRole("button", { name: "Edit “Mixed”" }).click();
  const kept = page.locator('[data-slot="kept-question"]');
  await expect(kept).toHaveCount(2);
  await expect(kept.nth(0)).toContainText(
    "“How deep?” is a scale question. This page cannot edit it yet; it is kept as it is.",
  );
  await expect(kept.nth(1)).toContainText("“In which order?” is a ranking question");
  await expect(page.locator('[data-slot="question-editor"]')).toContainText(
    "This question has settings this page does not show. They are kept.",
  );

  const patches: Record<string, unknown>[] = [];
  page.on("request", (request) => {
    if (request.method() === "PATCH") patches.push(request.postDataJSON());
  });
  await page.getByLabel("Title").fill("Mixed, renamed");
  await page.getByRole("button", { name: "Save changes" }).click();
  await expect(page.getByText("Saved “Mixed, renamed”.")).toBeVisible();
  // the questions were not sent at all, so none can have been rewritten
  expect(Object.keys(patches[0] ?? {}).sort()).toEqual(["revision", "title"]);
  const after = (await (await mine.call("GET", `/api/prompts/${made.id}`)).json()) as {
    variables: unknown;
  };
  expect(JSON.stringify(after.variables)).toBe(
    JSON.stringify((before as { variables: unknown }).variables),
  );

  // a change to a question the page edits sends the questions, and the others come back as they were
  await rowOf(page, "Mixed, renamed")
    .getByRole("button", { name: "Edit “Mixed, renamed”" })
    .click();
  await page.getByLabel("Question", { exact: true }).fill("What, exactly?");
  await page.getByRole("button", { name: "Save changes" }).click();
  await expect(page.getByText("Saved “Mixed, renamed”.").first()).toBeVisible();
  const changed = (await (await mine.call("GET", `/api/prompts/${made.id}`)).json()) as {
    variables: Record<string, unknown>[];
  };
  const was = (before as { variables: Record<string, unknown>[] }).variables;
  expect(changed.variables[0]).toEqual({ ...was[0], question: "What, exactly?" });
  expect(JSON.stringify(changed.variables[1])).toBe(JSON.stringify(was[1]));
  expect(JSON.stringify(changed.variables[2])).toBe(JSON.stringify(was[2]));
});

test("delete asks first, sends the revision, and a stale delete is told in words", async ({
  page,
  mine,
}) => {
  const one = await mine.seed({ name: "one", title: "One", body: "x" });
  const two = await mine.seed({ name: "two", title: "Two", body: "y" });
  const deletes: string[] = [];
  page.on("request", (request) => {
    if (request.method() === "DELETE") deletes.push(request.url());
  });
  await page.goto("/prompts");
  await rowOf(page, "One").getByRole("button", { name: "Delete “One”" }).click();
  const dialog = page.getByRole("alertdialog");
  await expect(dialog).toContainText("Delete “One”?");
  await expect(dialog).toContainText("Messages you already sent with it do not change");
  // cancelling sends nothing
  await dialog.getByRole("button", { name: "Cancel" }).click();
  expect(deletes).toEqual([]);
  await expect(rowOf(page, "One")).toBeVisible();

  // changed in another tab: the revision the page holds is stale
  const other = await mine.call("PATCH", `/api/prompts/${two.id}`, {
    revision: two.revision,
    title: "Two, renamed",
  });
  expect(other.status).toBe(200);
  await rowOf(page, "Two").getByRole("button", { name: "Delete “Two”" }).click();
  await page.getByRole("alertdialog").getByRole("button", { name: "Delete", exact: true }).click();
  await expect(page.getByRole("alertdialog")).toContainText(
    "It was changed since you opened this page",
  );
  expect(deletes.at(-1)).toContain(`/api/prompts/${two.id}?revision=${two.revision}`);
  // the list was read again: the row is what the other tab made it, and the dialog is still there to decide again
  await page.getByRole("alertdialog").getByRole("button", { name: "Cancel" }).click();
  await expect(rowOf(page, "Two, renamed")).toBeVisible();

  // and a delete from the fresh row is made
  await rowOf(page, "One").getByRole("button", { name: "Delete “One”" }).click();
  await page.getByRole("alertdialog").getByRole("button", { name: "Delete", exact: true }).click();
  await expect(rowOf(page, "One")).toHaveCount(0);
  expect(deletes.at(-1)).toContain(`/api/prompts/${one.id}?revision=${one.revision}`);
});

test("a prompt is shared with everyone and with roles; the row says who can read it, and only the audience is sent", async ({
  page,
  mine,
}) => {
  await mine.as("prompt-sharer");
  const made = await mine.seed(review());
  const patches: Record<string, unknown>[] = [];
  page.on("request", (request) => {
    if (request.method() === "PATCH") patches.push(request.postDataJSON());
  });
  await page.goto("/prompts");
  const row = rowOf(page, "Review a change");
  await expect(row).not.toContainText("Shared with:");

  await row.getByRole("button", { name: "Share “Review a change”" }).click();
  const dialog = page.getByRole("dialog");
  await expect(dialog.getByRole("heading", { name: "Share “Review a change”" })).toBeVisible();
  await expect(dialog.getByText("/review")).toBeVisible();
  const field = dialog.getByLabel("Roles who can read it");
  await expect(field).toHaveValue("");
  await expect(dialog.getByText("Only you can read it.", { exact: true })).toBeVisible();
  // `*` alone is everyone
  await field.fill("*");
  await expect(dialog.getByText("Everyone can read it.", { exact: true })).toBeVisible();
  await dialog.getByRole("button", { name: "Save" }).click();
  await expect(page.getByText("“Review a change” is shared with Everyone.")).toBeVisible();
  await expect(row).toContainText("Shared with: Everyone");
  // the revision it was opened at and the audience: no title, no text, no questions
  expect(patches).toHaveLength(1);
  expect(Object.keys(patches[0] ?? {}).sort()).toEqual(["audience", "revision"]);
  expect(patches[0]).toEqual({ revision: made.revision, audience: ["*"] });

  // a role, typed with spaces and twice: what the server stores is each once, sorted
  await row.getByRole("button", { name: "Change sharing “Review a change”" }).click();
  await expect(field).toHaveValue("*");
  await field.fill(" user , prompt-team, user ");
  await dialog.getByRole("button", { name: "Save" }).click();
  await expect(page.getByText("“Review a change” is shared with prompt-team, user.")).toBeVisible();
  await expect(row).toContainText("Shared with: prompt-team, user");
  expect(patches.at(-1)).toMatchObject({ audience: ["user", "prompt-team"] });
  const stored = (await (await mine.call("GET", "/api/prompts?scope=mine")).json()) as {
    items: { title: string; audience?: string[] }[];
  };
  expect(stored.items.find((p) => p.title === "Review a change")?.audience).toEqual([
    "prompt-team",
    "user",
  ]);
  await expectNoHorizontalScroll(page);
});

test("a person without prompts.share is offered no Share, and Stop sharing on what they shared before", async ({
  page,
  mine,
}) => {
  await mine.as("prompt-sharer");
  await mine.seed({ name: "priv", title: "Private one", body: "x" });
  await mine.seed({ name: "shared", title: "Shared one", body: "y", audience: ["user"] });
  await mine.share([{ name: "theirs", title: "Theirs", body: "z", audience: ["*"] }]);
  await page.goto("/prompts");
  await expect(
    rowOf(page, "Private one").getByRole("button", { name: "Share “Private one”" }),
  ).toBeVisible();
  await expect(
    rowOf(page, "Shared one").getByRole("button", { name: "Change sharing “Shared one”" }),
  ).toBeVisible();

  // the roles change: the same session is a plain user now, and the page reads who it is
  await mine.as("user");
  await page.reload();
  // the row is still there with the control it should have: a `toHaveCount(0)` scoped to a row that vanished proves nothing
  const priv = rowOf(page, "Private one");
  await expect(priv.getByRole("button", { name: "Edit “Private one”" })).toBeVisible();
  await expect(priv.getByRole("button", { name: /^Share/ })).toHaveCount(0);
  const frozen = rowOf(page, "Shared one");
  await expect(frozen.getByRole("button", { name: "Edit “Shared one”" })).toBeVisible();
  await expect(frozen.getByRole("button", { name: "Change sharing “Shared one”" })).toHaveCount(0);
  await frozen.getByRole("button", { name: "Stop sharing “Shared one”" }).click();
  await expect(page.getByText("“Shared one” is no longer shared.")).toBeVisible();
  await expect(frozen).not.toContainText("Shared with:");
  // and with the share gone, a person who may not share is offered nothing
  await expect(frozen.getByRole("button", { name: "Edit “Shared one”" })).toBeVisible();
  await expect(frozen.getByRole("button", { name: /^Share/ })).toHaveCount(0);
  const stored = (await (await mine.call("GET", "/api/prompts?scope=mine")).json()) as {
    items: { title: string; audience?: string[] }[];
  };
  expect(stored.items.find((p) => p.title === "Shared one")?.audience ?? []).toEqual([]);
  // a prompt shared with the person has no control at all
  const theirs = rowOf(page, "Theirs");
  await expect(theirs).toContainText("Shared with you");
  await expect(theirs.getByRole("button", { name: "View “Theirs”" })).toBeVisible();
  await expect(
    theirs.getByRole("button", { name: /^(Share|Change sharing|Stop sharing)/ }),
  ).toHaveCount(0);
});

test("bad_audience is said at the field, for * beside a role and for a role with a space", async ({
  page,
  mine,
}) => {
  await mine.as("prompt-sharer");
  await mine.seed(review());
  await page.goto("/prompts");
  await rowOf(page, "Review a change")
    .getByRole("button", { name: "Share “Review a change”" })
    .click();
  const field = page.getByLabel("Roles who can read it");

  for (const typed of ["* , user", "prompt team"]) {
    await field.fill(typed);
    await page.getByRole("button", { name: "Save" }).click();
    await expect(field).toBeFocused();
    await expect(field).toHaveAttribute("aria-invalid", "true");
    await expect(
      page.getByRole("alert").filter({ hasText: "A role is 1 to 64 characters with no spaces" }),
    ).toBeVisible();
    // the roles typed are still there
    await expect(field).toHaveValue(typed);
  }

  await field.fill("prompt-team");
  await page.getByRole("button", { name: "Save" }).click();
  await expect(page.getByText("“Review a change” is shared with prompt-team.")).toBeVisible();
});

test("a stale share keeps the roles typed, and Load the current version shows the current audience", async ({
  page,
  mine,
}) => {
  await mine.as("prompt-sharer");
  const made = await mine.seed(review());
  await page.goto("/prompts");
  await rowOf(page, "Review a change")
    .getByRole("button", { name: "Share “Review a change”" })
    .click();
  const field = page.getByLabel("Roles who can read it");
  await field.fill("user");
  // another tab shares it with a team: the revision the dialog holds is stale
  const other = await mine.call("PATCH", `/api/prompts/${made.id}`, {
    revision: made.revision,
    audience: ["team"],
  });
  expect(other.status).toBe(200);

  await page.getByRole("button", { name: "Save" }).click();
  await expect(errorLine(page)).toContainText("changed elsewhere since you opened it");
  await expect(field).toHaveValue("user");
  // again: still stale, never an overwrite
  await page.getByRole("button", { name: "Save" }).click();
  await expect(errorLine(page)).toContainText("Your changes are still here");
  const held = (await (await mine.call("GET", `/api/prompts/${made.id}`)).json()) as {
    audience: string[];
  };
  expect(held.audience).toEqual(["team"]);

  await page.getByRole("button", { name: "Load the current version" }).click();
  await expect(field).toHaveValue("team");
  await expect(page.getByRole("button", { name: "Load the current version" })).toHaveCount(0);
  await page.getByRole("button", { name: "Save" }).click();
  await expect(page.getByText("“Review a change” is shared with team.")).toBeVisible();
});

test("403 forbidden offers Stop sharing, the only write a person without prompts.share may make", async ({
  page,
  mine,
}) => {
  await mine.as("prompt-sharer");
  const made = await mine.seed(review({ audience: ["user"] }));
  await page.goto("/prompts");
  await rowOf(page, "Review a change")
    .getByRole("button", { name: "Change sharing “Review a change”" })
    .click();
  const dialog = page.getByRole("dialog");
  // the roles change behind the page's back: it still offers what it may no longer do
  await mine.as("user");
  await dialog.getByLabel("Roles who can read it").fill("user, prompt-team");
  await dialog.getByRole("button", { name: "Save" }).click();
  await expect(dialog).toContainText("Your roles no longer let you share prompts");

  await dialog.getByRole("button", { name: "Stop sharing" }).click();
  await expect(page.getByText("“Review a change” is no longer shared.")).toBeVisible();
  const held = (await (await mine.call("GET", `/api/prompts/${made.id}`)).json()) as {
    audience: string[];
  };
  expect(held.audience).toEqual([]);
});

test("on a phone the share dialog needs no sideways scroll, with a long list of roles", async ({
  page,
  mine,
}) => {
  await mine.as("prompt-sharer");
  const roles = Array.from({ length: 8 }, (_, i) => `a-long-role-name-number-${i}`);
  await mine.seed(review({ audience: roles }));
  await page.goto("/prompts");
  await expect(rowOf(page, "Review a change")).toContainText(
    "Shared with: a-long-role-name-number-0",
  );
  await expectNoHorizontalScroll(page);
  await rowOf(page, "Review a change")
    .getByRole("button", { name: "Change sharing “Review a change”" })
    .click();
  await expect(page.getByLabel("Roles who can read it")).toHaveValue(roles.join(", "));
  await expectNoHorizontalScroll(page);
});

test("at the limit New prompt goes away with the reason, and too_many_prompts from another tab says it", async ({
  page,
  mine,
}) => {
  test.setTimeout(120_000);
  const config = (await (await fetch(`${MOCK_URL}/api/config`)).json()) as {
    prompts: { limit: number };
  };
  const limit = config.prompts.limit;
  for (let i = 0; i < limit - 1; i++) {
    await mine.seed({ name: `p${i}`, title: `Prompt ${i}`, body: "x" });
  }
  await page.goto("/prompts");
  await expect(page.getByText(`(${limit - 1} of ${limit})`)).toBeVisible();
  await page.getByRole("button", { name: "New prompt" }).click();
  await page.getByLabel("Title").fill("The hundredth");
  await page.getByLabel("Text", { exact: true }).fill("x");
  // another tab takes the last place
  await mine.seed({ name: "taken", title: "Taken", body: "x" });
  await page.getByRole("button", { name: "Save prompt" }).click();
  await expect(errorLine(page)).toContainText(
    `You have ${limit} prompts, the most one person can have.`,
  );
  await expect(page.getByLabel("Title")).toHaveValue("The hundredth");
  await page.getByRole("button", { name: "Cancel" }).click();
  await expect(page.getByText(`(${limit} of ${limit})`)).toBeVisible();
  await expect(page.getByRole("button", { name: "New prompt" })).toHaveCount(0);
  await expect(
    page.getByText(
      `You have ${limit} prompts, the most one person can have. Delete one to add another.`,
    ),
  ).toBeVisible();
});

test("a 503 on the first read offers Try again; on a later read the list stays with a warning", async ({
  page,
  mine,
}) => {
  await mine.seed({ name: "kept", title: "Kept", body: "x" });
  let down = true;
  await page.route(/\/api\/prompts(\?.*)?$/, (route) =>
    down && route.request().method() === "GET"
      ? route.fulfill({
          status: 503,
          contentType: "application/problem+json",
          body: JSON.stringify({ title: "Unavailable", status: 503 }),
        })
      : route.continue(),
  );
  await page.goto("/prompts");
  await expect(errorLine(page)).toContainText(
    "Your prompts could not be loaded. Try again in a moment.",
  );
  await expect(page.getByRole("button", { name: "New prompt" })).toHaveCount(0);
  down = false;
  await page.getByRole("button", { name: "Try again" }).click();
  await expect(rowOf(page, "Kept")).toBeVisible();

  // a later read fails: the list stays, and says it may be out of date
  down = true;
  // (the page does not read twice within a quarter second)
  await page.waitForTimeout(400);
  await page.evaluate(() => window.dispatchEvent(new Event("focus")));
  await expect(
    page.getByText("The list could not be refreshed. What is shown may be out of date."),
  ).toBeVisible();
  await expect(rowOf(page, "Kept")).toBeVisible();
  down = false;
  await page.getByRole("button", { name: "Try again" }).click();
  await expect(page.getByText("The list could not be refreshed.")).toHaveCount(0);
});

test("without prompts in the configuration there is no menu item and the page says the library is not offered", async ({
  page,
  isMobile,
}) => {
  const openAccount = async () => {
    await page.goto("/");
    if (isMobile) {
      await page.getByRole("button", { name: "Threads", exact: true }).click();
      await expect(page.getByRole("dialog", { name: "Threads" })).toBeVisible();
    }
    await page.getByRole("button", { name: /^Account: / }).click();
  };
  await openAccount();
  await page.getByRole("menuitem", { name: "My prompts" }).click();
  await expect(page).toHaveURL(`${ORIGIN}/prompts`);
  await expect(page.getByRole("heading", { level: 1, name: "My prompts" })).toBeVisible();

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
  await openAccount();
  await expect(page.getByRole("menuitem", { name: "Usage" })).toBeVisible();
  await expect(page.getByRole("menuitem", { name: "My prompts" })).toHaveCount(0);
  await page.goto("/prompts");
  await expect(page.getByText("This deployment does not offer a prompt library.")).toBeVisible();
  await expect(page.getByRole("button", { name: "New prompt" })).toHaveCount(0);
  expect(asked).toEqual([]);
});

test("on a phone the rows and the form need no sideways scroll, with a long title, name and question", async ({
  page,
  mine,
}) => {
  const long = "A prompt with a long title ".repeat(4).trim();
  await mine.seed({
    name: "a-long-name-that-keeps-going-on",
    title: long,
    description: "A description that runs on and on. ".repeat(6),
    body: "Do {{x}}.",
    variables: [
      { id: "x", question: "A question that is also very long ".repeat(4).trim(), type: "text" },
    ],
  });
  await mine.share([
    {
      name: "shared-long",
      title: long,
      description: "words ".repeat(60),
      body: "w".repeat(300),
      audience: ["*"],
    },
  ]);
  await page.goto("/prompts");
  await expect(page.locator('[data-slot="prompt-row"]')).toHaveCount(2);
  await expectNoHorizontalScroll(page);
  await page
    .locator('[data-slot="prompt-row"][data-own="true"]')
    .getByRole("button", { name: /^Edit/ })
    .click();
  await expect(page.locator('[data-slot="question-editor"]')).toBeVisible();
  await expectNoHorizontalScroll(page);
  await page.getByLabel("Kind").selectOption("single");
  await expectNoHorizontalScroll(page);
  await page.getByRole("button", { name: "Cancel" }).click();
  await page.getByRole("button", { name: /^View/ }).click();
  await expectNoHorizontalScroll(page);
});

for (const scheme of ["light", "dark"] as const) {
  test.describe(`accessibility (${scheme})`, () => {
    test.use({ colorScheme: scheme, contextOptions: { reducedMotion: "reduce" } });

    async function serious(page: Page) {
      await animationsDone(page);
      const results = await new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa"]).analyze();
      return results.violations.filter((v) => v.impact === "serious" || v.impact === "critical");
    }

    test("axe: the list with own and shared, the empty list, the form with every editable kind and a kept one, a refused form, the delete dialog and the share dialog have no serious violations", async ({
      page,
      mine,
    }) => {
      await mine.as("prompt-sharer");
      await page.goto("/prompts");
      await expect(page.getByText("You have no prompts yet.")).toBeVisible();
      expect(await serious(page)).toEqual([]);

      await mine.seed(review());
      await mine.seed({
        name: "mixed",
        title: "Mixed",
        body: "{{a}} {{b}} {{c}} {{d}} {{e}} {{f}} {{g}} {{s}}",
        variables: [
          { id: "a", question: "A?", type: "text" },
          { id: "b", question: "B?", type: "longText" },
          { id: "c", question: "C?", options: FOCUS_CHOICES, allowOther: true },
          { id: "d", question: "D?", type: "multiple", options: FOCUS_CHOICES },
          { id: "e", question: "E?", type: "confirm", yesLabel: "Yes", noLabel: "No" },
          { id: "f", question: "F?", type: "number", minValue: "0", unit: "kg" },
          { id: "g", question: "G?", type: "date", minDate: "2026-01-01" },
          { id: "s", question: "S?", type: "scale", to: 5 },
        ],
      });
      await mine.share([
        { name: "shared", title: "Shared prompt", body: "Words.", audience: ["*"] },
      ]);
      await page.reload();
      await expect(page.locator('[data-slot="prompt-row"]')).toHaveCount(3);
      expect(await serious(page)).toEqual([]);

      // the form with every editable kind, and a kept one
      await rowOf(page, "Mixed").getByRole("button", { name: "Edit “Mixed”" }).click();
      await expect(page.locator('[data-slot="question-editor"]')).toHaveCount(7);
      await expect(page.locator('[data-slot="kept-question"]')).toHaveCount(1);
      expect(await serious(page)).toEqual([]);

      // a refused form
      await page.getByLabel("Text", { exact: true }).fill("{{a}} {{ b }}");
      await page.getByRole("button", { name: "Save changes" }).click();
      await expect(page.getByText(/These braces do not start a blank/)).toBeVisible();
      expect(await serious(page)).toEqual([]);
      await page.getByRole("button", { name: "Cancel" }).click();

      // the shared prompt, read only
      await rowOf(page, "Shared prompt")
        .getByRole("button", { name: "View “Shared prompt”" })
        .click();
      await expect(page.getByRole("heading", { name: "Shared prompt", level: 2 })).toBeVisible();
      expect(await serious(page)).toEqual([]);
      await page.getByRole("button", { name: "Back to your prompts" }).click();

      // the delete dialog
      await rowOf(page, "Mixed").getByRole("button", { name: "Delete “Mixed”" }).click();
      await expect(page.getByRole("alertdialog")).toContainText("Delete “Mixed”?");
      expect(await serious(page)).toEqual([]);
      await page.getByRole("alertdialog").getByRole("button", { name: "Cancel" }).click();

      // the share dialog, with a role typed
      await rowOf(page, "Review a change")
        .getByRole("button", { name: "Share “Review a change”" })
        .click();
      await expect(page.getByRole("dialog")).toBeVisible();
      await page.getByLabel("Roles who can read it").fill("user, prompt-team");
      await expect(page.getByText("Everyone with one of these roles can read it")).toBeVisible();
      expect(await serious(page)).toEqual([]);
    });
  });
}
