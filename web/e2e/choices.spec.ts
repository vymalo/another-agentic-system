import { expect, type Locator, type Page, test } from "@playwright/test";
import { badge, conversation, expectNoHorizontalScroll, startThread } from "./helpers";

/**
 * The Choices component (ADR 0023) against the mock orchestrator's `choices` story: one surface of
 * the web's own catalog with three questions, "Three questions", then the answers (one action, no
 * message) and the agent's echo of what was chosen.
 */

type Action = {
  messages: unknown[];
  resume?: unknown;
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

const ui = (page: Page) =>
  conversation(page).getByRole("region", { name: "Interface from reviewer" });
const send = (page: Page) => page.getByRole("button", { name: "Send answers" });
const db = (page: Page) => page.getByRole("radiogroup", { name: "Which database?" });
const login = (page: Page) => page.getByRole("radiogroup", { name: "Which login?" });
const where = (page: Page) => page.getByRole("group", { name: /Where does it run\?/ });

/**
 * What a person does: click the option's words (the whole tile is the label's target, so the
 * input itself sits under it). The check is on the input.
 */
async function pick(group: Locator, label: string, role: "radio" | "checkbox" = "radio") {
  await group.getByText(label, { exact: true }).click();
  await expect(group.getByRole(role, { name: label })).toBeChecked();
}

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

test.describe("Choices", () => {
  test("three questions are answered by clicking, one action goes out, and the agent continues", async ({
    page,
  }) => {
    const posts = runPosts(page);
    await startThread(page, "choices now", "Reviewer");
    await expect(badge(page)).toHaveText("Your turn");

    // the surface: a group per question, named by its question
    await expect(ui(page).getByText("A few quick choices")).toBeVisible();
    await expect(db(page).getByRole("radio")).toHaveCount(3); // Postgres, SQLite, Other
    await expect(login(page).getByRole("radio")).toHaveCount(2);
    await expect(where(page).getByRole("checkbox")).toHaveCount(3);
    await expect(page.getByText("Relational, the default")).toBeVisible();

    // nothing is sent until the click, and the button waits for the required questions
    await expect(send(page)).toBeDisabled();
    await expect(page.getByText("2 questions are not answered yet.")).toBeVisible();
    await pick(db(page), "Postgres");
    await expect(page.getByText("1 question is not answered yet.")).toBeVisible();
    await pick(login(page), "No login");
    await pick(where(page), "Docker Compose", "checkbox");
    await expect(send(page)).toBeEnabled();
    await page.waitForTimeout(300);
    expect(posts).toHaveLength(1);

    await send(page).click();
    await expect(badge(page)).toHaveText("Done");
    expect(posts).toHaveLength(2);
    const action = posts[1] as Action;
    expect(action.messages).toEqual([]);
    expect(action.resume).toBeUndefined();
    expect(action.forwardedProps.a2uiAction?.userAction).toMatchObject({
      name: "answer",
      surfaceId: "s1",
      sourceComponentId: "pick",
      context: {
        answers: [
          { id: "db", values: ["pg"] },
          { id: "auth", values: ["none"] },
          { id: "deploy", values: ["compose"] },
        ],
      },
    });

    // the answer is the person's: a bubble with the labels, not a step
    const bubble = page.locator('[data-slot="answer-bubble"]');
    await expect(bubble).toBeVisible();
    await expect(bubble.getByText("Your answers")).toBeVisible();
    expect(await bubbleLines(page)).toEqual([
      ["Which database?", "Postgres"],
      ["Which login?", "No login"],
      ["Where does it run?", "Docker Compose"],
    ]);
    await expect(conversation(page).locator('[data-slot="action-step"]')).toHaveCount(0);
    // the agent went on, with what was chosen
    await expect(
      conversation(page).getByText("answered: ui-action answer db=pg auth=none deploy=compose"),
    ).toHaveCount(1);

    // the Choices keeps its place and takes no more answers
    await expect(send(page)).toBeDisabled();
    await expect(db(page).getByRole("radio", { name: "Postgres" })).toBeDisabled();
    await expectNoHorizontalScroll(page);

    // a reload replays the same: the bubble, and a Choices that does not answer again
    await page.reload();
    await expect(badge(page)).toHaveText("Done");
    await expect(page.locator('[data-slot="answer-bubble"]')).toHaveCount(1);
    expect(await bubbleLines(page)).toEqual([
      ["Which database?", "Postgres"],
      ["Which login?", "No login"],
      ["Where does it run?", "Docker Compose"],
    ]);
    await expect(send(page)).toBeDisabled();
    await expect(page.locator('[data-slot="answer-bubble"]')).toHaveCount(1);
  });

  test("'Other' is the person's own words, sent as `other`, and shown as theirs", async ({
    page,
  }) => {
    const posts = runPosts(page);
    await startThread(page, "choices now", "Reviewer");
    await expect(badge(page)).toHaveText("Your turn");
    await page.getByLabel("Your own answer to: Which database?").fill("Cockroach");
    // typing turned 'Other' on, and answers the question
    await expect(db(page).getByRole("radio", { name: "Other" })).toBeChecked();
    await pick(login(page), "Keycloak");
    await send(page).click();
    await expect(badge(page)).toHaveText("Done");
    expect(posts[1]?.forwardedProps.a2uiAction?.userAction).toMatchObject({
      context: {
        answers: [
          { id: "db", values: [], other: "Cockroach" },
          { id: "auth" },
          { id: "deploy", values: [] },
        ],
      },
    });
    expect(await bubbleLines(page)).toEqual([
      ["Which database?", "Other: Cockroach"],
      ["Which login?", "Keycloak"],
      ["Where does it run?", "No answer"],
    ]);
    await expect(
      conversation(page).getByText(
        "answered: ui-action answer db=other:Cockroach auth=keycloak deploy=",
      ),
    ).toHaveCount(1);
  });

  test("from the keyboard alone: arrows choose, space ticks, Enter in a text box sends nothing", async ({
    page,
    isMobile,
  }) => {
    test.skip(isMobile, "no keyboard on a phone");
    const posts = runPosts(page);
    await startThread(page, "choices now", "Reviewer");
    await expect(badge(page)).toHaveText("Your turn");

    // the first group is one tab stop; space chooses, the arrows move the choice
    await db(page).getByRole("radio", { name: "Postgres" }).focus();
    await page.keyboard.press("Space");
    await expect(db(page).getByRole("radio", { name: "Postgres" })).toBeChecked();
    await page.keyboard.press("ArrowDown");
    await expect(db(page).getByRole("radio", { name: "SQLite" })).toBeChecked();
    await expect(db(page).getByRole("radio", { name: "Postgres" })).not.toBeChecked();

    // the second group: focus it and choose with the arrows
    await login(page).getByRole("radio", { name: "Keycloak" }).focus();
    await page.keyboard.press("ArrowDown");
    await expect(login(page).getByRole("radio", { name: "No login" })).toBeChecked();

    // a check box is ticked with the space bar
    await where(page).getByRole("checkbox", { name: "Kubernetes" }).focus();
    await page.keyboard.press("Space");
    await expect(where(page).getByRole("checkbox", { name: "Kubernetes" })).toBeChecked();

    // Enter in the box of an 'Other' is a line break of nothing: it sends nothing
    const own = page.getByLabel("Your own answer to: Where does it run?");
    await own.focus();
    await page.keyboard.type("bare metal");
    await page.keyboard.press("Enter");
    await page.waitForTimeout(300);
    expect(posts).toHaveLength(1);

    // the button is the next stop, and takes Enter
    await page.keyboard.press("Tab");
    await expect(send(page)).toBeFocused();
    await page.keyboard.press("Enter");
    await expect(badge(page)).toHaveText("Done");
    expect(posts[1]?.forwardedProps.a2uiAction?.userAction).toMatchObject({
      context: {
        answers: [
          { id: "db", values: ["sqlite"] },
          { id: "auth", values: ["none"] },
          { id: "deploy", values: ["k8s"], other: "bare metal" },
        ],
      },
    });
  });
});
