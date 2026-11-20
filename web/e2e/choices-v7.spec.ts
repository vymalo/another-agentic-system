import { expect, type Page, test } from "@playwright/test";
import { badge, conversation, expectNoHorizontalScroll, startThread } from "./helpers";

/**
 * The five kinds of question of catalog version 7 (docs/api/ui-catalog-v1.md, "Version 7") against
 * the mock orchestrator's `choices-v7` story: one form of a yes or no, a number, a scale, a day and
 * a list to put in order, answered with the keyboard; the client says why a bad answer cannot go;
 * a receiver that is handed a bad answer anyway refuses it by naming the question and the rule and
 * asks again; and a form of version 5 is drawn as it always was.
 */

type Action = {
  messages: unknown[];
  forwardedProps: { a2uiAction?: { userAction: Record<string, unknown> } };
};

/** The run POSTs the page makes. */
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
const ship = (page: Page) => page.getByRole("radiogroup", { name: "Delete the old branch?" });
const nps = (page: Page) =>
  page.getByRole("radiogroup", { name: "How likely are you to recommend it?" });
const budget = (page: Page) => page.getByRole("textbox", { name: /What is the monthly budget/ });
const when = (page: Page) => page.getByLabel(/When should it ship/);
const rank = (page: Page) => page.getByRole("list", { name: /Put these in order of importance/ });
/** The line of words under the number box, and the one under the date box (both are always in the page). */
const problems = (page: Page) => page.locator('[data-slot="choices-problem"]');
const numberProblem = (page: Page) => problems(page).first();
const dateProblem = (page: Page) => problems(page).last();

/** What a person does: click the option's words (the whole tile is the label's target). */
async function choose(page: Page, label: string) {
  await page.getByText(label, { exact: true }).click();
  await expect(page.getByRole("radio", { name: label })).toBeChecked();
}
const items = (page: Page) => rank(page).locator('[data-slot="choices-rank-label"]');

/** What the bubble says: `[question, answer]` per line. */
async function bubbleLines(page: Page): Promise<string[][]> {
  return page
    .locator('[data-slot="answer-line"]')
    .evaluateAll((lines) =>
      lines.map((l) => [
        l.querySelector("dt")?.textContent ?? "",
        l.querySelector("dd")?.textContent ?? "",
      ]),
    );
}

test.describe("the five kinds of question (catalog version 7)", () => {
  test("each is answered by keyboard, one action goes out, and the answer reads back as the person's", async ({
    page,
  }) => {
    const posts = runPosts(page);
    await startThread(page, "choices-v7 now", "Reviewer");
    await expect(badge(page)).toHaveText("Your turn");
    await expect(page.getByRole("heading", { name: "Before I ship" })).toBeVisible();

    // confirm: a radio group of the agent's two phrases, the safe one chosen; the arrows change it
    await expect(ship(page).getByRole("radio")).toHaveCount(2);
    await expect(ship(page).getByRole("radio", { name: "Keep it" })).toBeChecked();
    await ship(page).getByRole("radio", { name: "Keep it" }).focus();
    await page.keyboard.press("ArrowLeft");
    await expect(ship(page).getByRole("radio", { name: "Delete it" })).toBeChecked();
    await page.keyboard.press("ArrowRight");
    await expect(ship(page).getByRole("radio", { name: "Keep it" })).toBeChecked();

    // the form waits for what is not answered, and says how many
    await expect(send(page)).toBeDisabled();
    await expect(page.getByText("3 questions are not answered yet.")).toBeVisible();

    // number: Tab from the radio group lands in the box; the person types
    await page.keyboard.press("Tab");
    await expect(budget(page)).toBeFocused();
    await page.keyboard.type("250.5");
    await expect(problems(page)).toHaveText(["", ""]);

    // scale: the points are a radio group; the arrows walk it, from the first to the seventh
    await nps(page).getByRole("radio", { name: "0", exact: true }).focus();
    await page.keyboard.press("Space");
    for (let i = 0; i < 7; i++) await page.keyboard.press("ArrowRight");
    await expect(nps(page).getByRole("radio", { name: "7", exact: true })).toBeChecked();
    await expect(page.getByText("Not at all")).toBeVisible();
    await expect(page.getByText("Certainly")).toBeVisible();

    // date: the browser's own control. A native date box is typed in its own segments, which differ by
    // browser and locale (Chromium mis-places the year digits when the box has a min and a max), so the
    // day is set as a value: what the page must send is the control's `YYYY-MM-DD`
    await when(page).focus();
    await when(page).fill("2026-12-15");
    await expect(when(page)).toHaveValue("2026-12-15");
    await expect(when(page)).toHaveAttribute("min", "2026-12-01");
    await expect(when(page)).toHaveAttribute("max", "2026-12-31");

    // rank: Move up and Move down are buttons; Enter presses them and the keyboard keeps its place
    await expect(items(page)).toHaveText(["Speed", "Cost", "Risk"]);
    const riskUp = page.getByRole("button", { name: "Move Risk up" });
    await riskUp.focus();
    await page.keyboard.press("Enter");
    await expect(items(page)).toHaveText(["Speed", "Risk", "Cost"]);
    await expect(riskUp).toBeFocused();
    await expect(page.getByRole("status").filter({ hasText: "Risk, 2 of 3" })).toBeAttached();
    await page.keyboard.press("Enter");
    await expect(items(page)).toHaveText(["Risk", "Speed", "Cost"]);
    await expect(riskUp).toBeFocused();
    await expect(riskUp).toHaveAttribute("aria-disabled", "true");

    // nothing was sent while the person worked
    await expect(send(page)).toBeEnabled();
    await page.waitForTimeout(300);
    expect(posts).toHaveLength(1);
    await expectNoHorizontalScroll(page);

    await send(page).focus();
    await page.keyboard.press("Enter");
    await expect(badge(page)).toHaveText("Done");
    expect(posts).toHaveLength(2);
    const action = posts[1] as Action;
    expect(action.messages).toEqual([]);
    expect(action.forwardedProps.a2uiAction?.userAction).toMatchObject({
      name: "answer",
      surfaceId: "s1",
      sourceComponentId: "pick",
      context: {
        answers: [
          { id: "ship", values: ["no"] },
          { id: "budget", values: ["250.5"] },
          { id: "nps", values: ["7"] },
          { id: "when", values: ["2026-12-15"] },
          { id: "order", values: ["risk", "speed", "cost"] },
        ],
      },
    });

    // the answer is the person's: the labels they saw, the rank as a numbered list
    await expect(page.locator('[data-slot="answer-bubble"]')).toBeVisible();
    expect(await bubbleLines(page)).toEqual([
      ["Delete the old branch?", "Keep it"],
      ["What is the monthly budget?", "250.5 EUR"],
      ["How likely are you to recommend it?", "7"],
      ["When should it ship?", "December 15, 2026"],
      ["Put these in order of importance", "RiskSpeedCost"],
    ]);
    await expect(page.locator('[data-slot="answer-rank"] li')).toHaveText([
      "Risk",
      "Speed",
      "Cost",
    ]);
    await expect(
      conversation(page).getByText(
        "answered: ui-action answer ship=no budget=250.5 nps=7 when=2026-12-15 order=risk,speed,cost",
      ),
    ).toHaveCount(1);

    // the form keeps its place, as it was answered, and takes no more input
    await expect(send(page)).toBeDisabled();
    await expect(budget(page)).toBeDisabled();
    await expect(budget(page)).toHaveValue("250.5");
    await expect(page.getByRole("button", { name: "Move Cost down" })).toBeDisabled();
    await expectNoHorizontalScroll(page);

    // a reload replays the same
    await page.reload();
    await expect(page.locator('[data-slot="answer-bubble"]')).toBeVisible();
    await expect(send(page)).toBeDisabled();
  });

  test("the focus stays on the Move button that was used, up and down, and an end button says so in words", async ({
    page,
  }) => {
    await startThread(page, "choices-v7 now", "Reviewer");
    await expect(badge(page)).toHaveText("Your turn");
    const focusedName = () =>
      page.evaluate(() => document.activeElement?.getAttribute("aria-label") ?? "");
    const status = page.getByRole("status").filter({ hasText: /of 3|already/ });

    const down = page.getByRole("button", { name: "Move Cost down" });
    await down.focus();
    await page.keyboard.press("Enter");
    await expect(items(page)).toHaveText(["Speed", "Risk", "Cost"]);
    expect(await focusedName()).toBe("Move Cost down");
    await expect(status).toHaveText("Cost, 3 of 3");

    const up = page.getByRole("button", { name: "Move Cost up" });
    await up.focus();
    await page.keyboard.press("Enter");
    await expect(items(page)).toHaveText(["Speed", "Cost", "Risk"]);
    expect(await focusedName()).toBe("Move Cost up");

    // the end of the list: nothing moves, the words say so, and say it again the second time
    const top = page.getByRole("button", { name: "Move Speed up" });
    await top.focus();
    await page.keyboard.press("Enter");
    await expect(status).toHaveText(/^Speed is already first/);
    const first = await status.textContent();
    await page.keyboard.press("Enter");
    await expect.poll(async () => await status.textContent()).not.toBe(first);
    expect(await focusedName()).toBe("Move Speed up");
  });

  test("a form that has no answer yet says what it needs, and the held button stays in the tab order", async ({
    page,
  }) => {
    await startThread(page, "choices-v7 now", "Reviewer");
    await expect(badge(page)).toHaveText("Your turn");
    await expect(page.getByText("Enter a number.")).toBeVisible();
    await expect(page.getByText("Pick a day.")).toBeVisible();
    await expect(page.getByText("Move an item, or keep the order, to answer.")).toBeVisible();
    // a held button can be focused, so that its reason is read
    await send(page).focus();
    await expect(send(page)).toBeFocused();
    await expect(send(page)).toHaveAttribute("aria-disabled", "true");
    // an optional scale that was answered can be cleared
    await page.getByText("7", { exact: true }).click();
    await page
      .getByRole("button", { name: "Clear answer to How likely are you to recommend it?" })
      .click();
    await expect(nps(page).getByRole("radio", { checked: true })).toHaveCount(0);
  });

  test("an out-of-range number cannot be sent and says why, in words, until it is right", async ({
    page,
  }) => {
    const posts = runPosts(page);
    await startThread(page, "choices-v7 now", "Reviewer");
    await expect(badge(page)).toHaveText("Your turn");
    // everything else is answered, so that only the number stands in the way
    await choose(page, "Delete it");
    await when(page).fill("2026-12-15");
    await page.getByRole("button", { name: "Keep this order" }).click();

    for (const [typed, said] of [
      ["1500", "The most is 1000 EUR."],
      ["-1", "The least is 0 EUR."],
      ["250.25", "Use steps of 0.5 EUR from 0."],
      ["1e3", "Use digits and a point"],
      ["1,5", "Use digits and a point"],
      ["12.5.1", "Use digits and a point"],
    ] as const) {
      await budget(page).fill(typed);
      await expect(send(page)).toBeDisabled();
      await expect(numberProblem(page)).toContainText(said);
      await expect(budget(page)).toHaveAttribute("aria-invalid", "true");
      // the words are linked to the box, so that a screen reader reads them with it
      const id = await numberProblem(page).getAttribute("id");
      await expect(budget(page)).toHaveAttribute("aria-describedby", new RegExp(`${id}`));
    }
    // a day outside the bounds is held the same way
    await budget(page).fill("999.5");
    await expect(send(page)).toBeEnabled();
    await when(page).fill("2027-01-02");
    await expect(send(page)).toBeDisabled();
    await expect(dateProblem(page)).toContainText("The latest day is 2026-12-31.");
    await when(page).fill("2026-12-31");
    await expect(send(page)).toBeEnabled();
    await expect(problems(page)).toHaveText(["", ""]);
    await page.waitForTimeout(300);
    expect(posts).toHaveLength(1);
  });

  test("a rank is moved with the buttons and sends the ids in that order; keeping the order is an answer too", async ({
    page,
  }) => {
    const posts = runPosts(page);
    await startThread(page, "choices-v7 now", "Reviewer");
    await expect(badge(page)).toHaveText("Your turn");
    await choose(page, "Delete it");
    await budget(page).fill("10");
    await when(page).fill("2026-12-02");
    // an untouched rank is not an answer: the agent's order is not the person's ranking
    await expect(send(page)).toBeDisabled();
    await expect(page.getByText("Move an item, or keep the order, to answer.")).toBeVisible();

    await page.getByRole("button", { name: "Move Speed down" }).click();
    await page.getByRole("button", { name: "Move Speed down" }).click();
    await expect(items(page)).toHaveText(["Cost", "Risk", "Speed"]);
    await expect(page.getByRole("button", { name: "Keep this order" })).toHaveCount(0);
    // the two ends do nothing when pressed: they say they are unavailable and keep the keyboard's place
    for (const name of ["Move Cost up", "Move Speed down"]) {
      const end = page.getByRole("button", { name });
      await expect(end).toHaveAttribute("aria-disabled", "true");
      await end.focus();
      await page.keyboard.press("Enter");
      await expect(end).toBeFocused();
    }
    await expect(items(page)).toHaveText(["Cost", "Risk", "Speed"]);

    await send(page).click();
    await expect(badge(page)).toHaveText("Done");
    expect(
      (posts[1] as Action).forwardedProps.a2uiAction?.userAction.context as {
        answers: { id: string; values: string[] }[];
      },
    ).toMatchObject({
      answers: [
        { id: "ship", values: ["yes"] },
        { id: "budget", values: ["10"] },
        { id: "nps", values: [] },
        { id: "when", values: ["2026-12-02"] },
        { id: "order", values: ["cost", "risk", "speed"] },
      ],
    });
    await expect(
      conversation(page).getByText(
        "answered: ui-action answer ship=yes budget=10 nps= when=2026-12-02 order=cost,risk,speed",
      ),
    ).toHaveCount(1);
  });

  test("a receiver that is handed a bad answer anyway refuses it by naming the question and the rule, and asks again", async ({
    page,
  }) => {
    // a hand-made request: the page would not send it, so it is changed on its way out
    await page.route("**/agui/agents/**", async (route) => {
      const body =
        route.request().method() === "POST"
          ? (route.request().postDataJSON() as Action | null)
          : null;
      const action = body?.forwardedProps?.a2uiAction?.userAction;
      const context = action?.context as
        | { answers: { id: string; values: string[] }[] }
        | undefined;
      const first = context?.answers.find((a) => a.id === "budget");
      if (action?.surfaceId === "s1" && first) {
        first.values = ["1500"];
        await route.continue({ postData: JSON.stringify(body) });
      } else {
        await route.continue();
      }
    });
    const posts = runPosts(page);
    await startThread(page, "choices-v7 now", "Reviewer");
    await expect(badge(page)).toHaveText("Your turn");
    await choose(page, "Delete it");
    await budget(page).fill("250.5");
    await when(page).fill("2026-12-15");
    await page.getByRole("button", { name: "Move Risk up" }).click();
    await send(page).click();

    // the agent says which question and which rule, and does not go on
    await expect(
      conversation(page).getByText(/The answer to "budget" was not used \(above_max\)/),
    ).toBeVisible();
    await expect(badge(page)).toHaveText("Your turn");
    await expect(conversation(page).getByText(/answered:/)).toHaveCount(0);

    // a new form, with what was valid kept: the choice, the day and the order, and no number
    const forms = conversation(page).getByRole("region", { name: "Interface from reviewer" });
    const again = forms.last();
    await expect(again.getByRole("radio", { name: "Delete it" })).toBeChecked();
    await expect(again.getByRole("textbox", { name: /monthly budget/ })).toHaveValue("");
    await expect(again.getByLabel(/When should it ship/)).toHaveValue("2026-12-15");
    await expect(again.locator('[data-slot="choices-rank-label"]')).toHaveText([
      "Speed",
      "Risk",
      "Cost",
    ]);
    // the first form is answered and stays as it was
    await expect(forms.first().getByRole("textbox", { name: /monthly budget/ })).toBeDisabled();

    await again.getByRole("textbox", { name: /monthly budget/ }).fill("999.5");
    // the order it starts in is the person's earlier one, and still no answer until they say so
    await expect(again.getByRole("button", { name: "Send answers" })).toHaveAttribute(
      "aria-disabled",
      "true",
    );
    await again.getByRole("button", { name: "Keep this order" }).click();
    await again.getByRole("button", { name: "Send answers" }).click();
    await expect(badge(page)).toHaveText("Done");
    expect(posts).toHaveLength(3);
    await expect(
      conversation(page).getByText(
        "answered: ui-action answer ship=yes budget=999.5 nps= when=2026-12-15 order=speed,risk,cost",
      ),
    ).toHaveCount(1);
  });

  test("a form of version 5 is drawn as it was: no control of the new kinds, the same answer", async ({
    page,
  }) => {
    const posts = runPosts(page);
    await startThread(page, "choices-v5 now", "Reviewer");
    await expect(badge(page)).toHaveText("Your turn");
    await expect(page.getByRole("radiogroup", { name: "Which database?" })).toBeVisible();
    await expect(page.locator("[data-kind]")).toHaveCount(0);
    await page.getByText("Development", { exact: true }).click();
    await page.getByLabel("What is the project called?").fill("Quokka");
    await page.getByRole("button", { name: "Start the migration" }).click();
    await expect(badge(page)).toHaveText("Done");
    expect(
      (posts[1] as Action).forwardedProps.a2uiAction?.userAction.context as {
        answers: unknown[];
      },
    ).toMatchObject({
      answers: [
        { id: "db", values: ["pg"] },
        { id: "envs", values: ["dev"] },
        { id: "name", values: [], text: "Quokka" },
        { id: "story", values: [] },
      ],
    });
  });
});
