import { expect, type Locator, type Page, test } from "@playwright/test";
import { badge, conversation, expectNoHorizontalScroll, startThread } from "./helpers";

/**
 * A question that shows what it asks about (docs/api/ui-catalog-v1.md, "Version 8", "A question
 * shows") against the mock orchestrator's `choices-v8` story: the coder shares a screenshot and asks two
 * questions, the first showing the screenshot and a diagram before its radio buttons, the second showing
 * two cards and a chart before its two choices. What is shown is part of the question's group, so a
 * screen reader meets it after the question and before the control; and a surface that breaks a rule of
 * version 8 is refused whole, in words, by the rule the contract names.
 */

type Action = {
  messages: unknown[];
  forwardedProps: { a2uiAction?: { userAction: Record<string, unknown> } };
};

function runPosts(page: Page): Action[] {
  const bodies: Action[] = [];
  page.on("request", (r) => {
    if (r.method() === "POST" && r.url().includes("/agui/agents/")) {
      bodies.push(r.postDataJSON() as Action);
    }
  });
  return bodies;
}

const send = (page: Page) => page.getByRole("button", { name: "Send answers" });
const layout = (page: Page) =>
  page.getByRole("radiogroup", { name: "Which layout should I keep?" });
const ship = (page: Page) => page.getByRole("radiogroup", { name: "Ship the list behind a flag?" });

/** The browser decoded the picture: it has a size, so it is a picture and not a broken icon. */
async function expectDecoded(img: Locator) {
  await expect(img).toBeVisible();
  await expect
    .poll(() =>
      img.evaluate(
        (el) => (el as HTMLImageElement).complete && (el as HTMLImageElement).naturalWidth,
      ),
    )
    .toBeGreaterThan(0);
}

/** The positions of these things in the order of the page, by the text a screen reader would read first. */
async function inOrder(group: Locator, selectors: string[]): Promise<boolean> {
  return group.evaluate((el, sels) => {
    const found = sels.map((s) => el.querySelector(s));
    if (found.some((f) => f === null)) return false;
    return found.every(
      (node, i) =>
        i === 0 ||
        Boolean(
          (found[i - 1] as Node).compareDocumentPosition(node as Node) &
            Node.DOCUMENT_POSITION_FOLLOWING,
        ),
    );
  }, selectors);
}

test.describe("a question that shows what it asks about (catalog version 8)", () => {
  test("shows a picture of the thread's file and a diagram inside the question's group, before its radio buttons", async ({
    page,
  }) => {
    await startThread(page, "choices-v8 now", "Reviewer");
    await expect(badge(page)).toHaveText("Your turn");
    const ui = conversation(page).getByRole("region", { name: "Interface from reviewer" });
    await expect(
      ui.getByRole("heading", { name: "Before I build the people screen" }),
    ).toBeVisible();

    // the picture: the thread's own file, by its route, named by its alt, and inside the group
    const picture = layout(page).getByRole("img", {
      name: "The people screen today: a list of four people with an avatar each",
    });
    await expectDecoded(picture);
    expect(await picture.getAttribute("src")).toMatch(
      /^\/api\/threads\/[0-9a-f-]{36}\/artifacts\/[0-9a-f]{64}$/,
    );
    await expect(layout(page).getByText("The people screen today", { exact: true })).toBeVisible();

    // the diagram: drawn as an image, with its source one click away as its text alternative
    const diagram = layout(page).locator('[data-slot="mermaid-image"]');
    await expectDecoded(diagram);
    await expect(diagram).toHaveAttribute("alt", "How a request reaches the screen");
    await expect(layout(page).locator('[data-slot="mermaid-source"]')).toBeVisible();

    // the order a screen reader meets them: the words of the question, its help, the picture, the diagram, the control
    expect(
      await inOrder(layout(page), [
        "legend",
        "p.text-xs",
        '[data-slot="choices-shown"] img',
        '[data-slot="mermaid-image"]',
        'input[type="radio"]',
      ]),
    ).toBe(true);
    const aria = await layout(page).ariaSnapshot();
    const at = (s: string) => aria.indexOf(s);
    expect(at("Which layout should I keep?")).toBeGreaterThanOrEqual(0);
    expect(at("The people screen today: a list")).toBeGreaterThan(
      at("Which layout should I keep?"),
    );
    expect(at("How a request reaches the screen")).toBeGreaterThan(
      at("The people screen today: a list"),
    );
    expect(at('radio "The list"')).toBeGreaterThan(at("How a request reaches the screen"));

    // and they are drawn once, nowhere else
    await expect(page.getByRole("img", { name: /^The people screen today/ })).toHaveCount(1);
    await expectNoHorizontalScroll(page);
  });

  test("the second question shows two cards and a chart before its yes or no, in its own group", async ({
    page,
  }) => {
    await startThread(page, "choices-v8 now", "Reviewer");
    await expect(badge(page)).toHaveText("Your turn");
    await expect(ship(page)).toBeVisible();
    await expect(ship(page).getByText("The two ways to ship it")).toBeVisible();
    await expect(ship(page).getByText("Ten percent of people see it first.")).toBeVisible();
    await expect(ship(page).getByText("Runs per day", { exact: true })).toBeVisible();
    // the chart keeps its data one click away, as everywhere
    await expect(ship(page).getByText("Data", { exact: true })).toBeVisible();
    // none of it is in the other question's group
    await expect(layout(page).getByText("Runs per day", { exact: true })).toHaveCount(0);
    await expect(layout(page).getByText("The two ways to ship it")).toHaveCount(0);
    await expect(ship(page).getByRole("radio")).toHaveCount(2);
    await expect(ship(page).getByRole("radio", { name: "Behind a flag" })).toBeChecked();
    await expectNoHorizontalScroll(page);
  });

  test("is answered by keyboard, one action goes out, and what was shown does not change it", async ({
    page,
  }) => {
    const posts = runPosts(page);
    await startThread(page, "choices-v8 now", "Reviewer");
    await expect(badge(page)).toHaveText("Your turn");
    await layout(page).getByRole("radio", { name: "The list" }).focus();
    await page.keyboard.press("Space");
    await expect(layout(page).getByRole("radio", { name: "The list" })).toBeChecked();
    await page.keyboard.press("ArrowDown");
    await expect(layout(page).getByRole("radio", { name: "The matches" })).toBeChecked();
    await page.keyboard.press("ArrowUp");
    await expect(send(page)).toBeEnabled();
    await send(page).click();
    await expect(badge(page)).toHaveText("Done");
    const actions = posts.filter((p) => p.forwardedProps.a2uiAction);
    expect(actions).toHaveLength(1);
    expect(actions[0]?.forwardedProps.a2uiAction?.userAction).toMatchObject({
      name: "answer",
      sourceComponentId: "pick",
      context: {
        answers: [
          { id: "layout", values: ["list"] },
          { id: "ship", values: ["yes"] },
        ],
      },
    });
    await expect(conversation(page)).toContainText(
      "answered: ui-action answer layout=list ship=yes",
    );
    // read back as the person's answer, with the labels they saw
    const lines = await page
      .locator('[data-slot="answer-line"]')
      .evaluateAll((ls) => ls.map((l) => l.textContent ?? ""));
    expect(lines.join("|")).toContain("The list");
    expect(lines.join("|")).toContain("Behind a flag");
  });
});

test.describe("a surface that breaks a rule of version 8 is refused whole", () => {
  test("a question that shows another form is refused (schema), one whose shown text is also placed too (expansion)", async ({
    page,
  }) => {
    await startThread(page, "choices-v8-bad now", "Reviewer");
    await expect(badge(page)).toHaveText("Done");
    const refusals = conversation(page).locator("[data-rule]");
    await expect(refusals).toHaveCount(2);
    await expect(refusals.nth(0)).toHaveAttribute("data-rule", "schema");
    await expect(refusals.nth(0)).toContainText("is shown by the question");
    await expect(refusals.nth(0)).toContainText("which may show only");
    await expect(refusals.nth(1)).toHaveAttribute("data-rule", "expansion");
    await expect(refusals.nth(1)).toContainText("is also placed in the surface");
    // never half drawn: none of the questions is in the page
    // (the raw operations of a refusal hold the words; what is drawn is a form, a group named by them)
    await expect(page.getByRole("radiogroup")).toHaveCount(0);
    await expect(page.getByRole("textbox")).toHaveCount(1); // the message box, and no question's
    await expect(page.getByRole("button", { name: "Send answers" })).toHaveCount(0);
    await expectNoHorizontalScroll(page);
  });
});

test.describe("a form of version 7 is drawn as it was", () => {
  test("a question that shows nothing has no shown block, and is the group it always was", async ({
    page,
  }) => {
    await startThread(page, "choices-v7 now", "Reviewer");
    await expect(badge(page)).toHaveText("Your turn");
    const group = page.getByRole("radiogroup", { name: "Delete the old branch?" });
    await expect(group).toBeVisible();
    await expect(page.locator('[data-slot="choices-shown"]')).toHaveCount(0);
    expect(await group.evaluate((el) => el.tagName)).toBe("FIELDSET");
    await expect(group.getByRole("radio")).toHaveCount(2);
    await expect(group.getByRole("radio", { name: "Keep it" })).toBeChecked();
    await expectNoHorizontalScroll(page);
  });
});
