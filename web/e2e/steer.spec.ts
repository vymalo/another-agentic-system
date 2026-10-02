import AxeBuilder from "@axe-core/playwright";
import { expect, type Page, test } from "@playwright/test";
import { badge, conversation, expectNoHorizontalScroll, MOCK_URL, startThread } from "./helpers";

/*
 * Sending while the agent works (ADR 0036, web/DESIGN.md "Sending while the agent works"), against
 * the mock: Send (steer), Stop and send (interrupt), the keys, the note under the bubble, and the
 * guard that holds a send back until the conversation is on screen. `gate …` holds the run until the
 * test releases it, `slow …` works until it is stopped (mock/scripts.ts). The mock lists steer/v1 for
 * the Coder, not for the Reviewer, so both wordings are seen.
 */

const box = (page: Page) => page.getByLabel("Message");
const send = (page: Page) => page.getByRole("button", { name: "Send" });
const more = (page: Page) => page.getByRole("button", { name: "Delivery options" });
const notes = (page: Page) => conversation(page).locator('[data-slot="delivery-note"]');

const threadId = (page: Page) => /\/threads\/([0-9a-f-]{36})$/.exec(page.url())?.[1] ?? "";

async function release(page: Page) {
  const res = await fetch(`${MOCK_URL}/__mock/release?thread=${threadId(page)}`, {
    method: "POST",
  });
  expect(res.status).toBe(204);
}

/** The bodies of the runs the page posted, as they go out. */
function posted(page: Page) {
  const bodies: { messages: { content: string }[]; forwardedProps: Record<string, unknown> }[] = [];
  page.on("request", (r) => {
    if (r.method() === "POST" && /\/agui\/agents\//.test(r.url())) {
      bodies.push(r.postDataJSON());
    }
  });
  return bodies;
}

/** A thread that is working, drawn, and ready to be sent to. */
async function working(page: Page, text: string, agent?: string) {
  await startThread(page, text, agent);
  await expect(badge(page)).toHaveText("Working…");
  await expect(conversation(page).getByText(text, { exact: true })).toBeVisible();
}

async function axeViolations(page: Page) {
  // axe reads the colours as they are drawn: a menu that is fading in is not at its colours yet
  // (its muted text measured 4.49:1 inside the fade on a loaded machine)
  await page.evaluate(() =>
    Promise.all(
      document
        .getAnimations()
        .filter((a) => a.effect?.getComputedTiming().iterations !== Number.POSITIVE_INFINITY)
        .map((a) => a.finished.catch(() => undefined)),
    ),
  );
  const results = await new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa"]).analyze();
  return results.violations.filter((v) => v.impact === "serious" || v.impact === "critical");
}

test("Send: the message goes to the working agent and the bubble says it was sent while it worked", async ({
  page,
}) => {
  const bodies = posted(page);
  await working(page, "gate refactor the parser");
  // the box stays open, with Stop; Send appears with the text
  await expect(box(page)).toBeEnabled();
  await expect(page.getByRole("button", { name: "Stop" })).toBeVisible();
  await expect(send(page)).toHaveCount(0);
  await box(page).fill("echo you were wrong since line 1");
  await expect(send(page)).toBeEnabled();
  await expect(send(page)).toHaveAccessibleDescription("Coder reads it at its next step");
  await send(page).click();

  await expect(notes(page)).toHaveText("Sent while Coder was working · read at its next step");
  expect(bodies).toHaveLength(2);
  expect(bodies[1]?.messages).toMatchObject([{ content: "echo you were wrong since line 1" }]);
  expect(bodies[1]?.forwardedProps["vymalo.send"]).toBe("steer");
  // the first turn is still there, once, and the box is empty and focused for the next one
  await expect(
    conversation(page).getByText("gate refactor the parser", { exact: true }),
  ).toHaveCount(1);
  await expect(box(page)).toHaveValue("");
  await expect(box(page)).toBeFocused();
  await expect(badge(page)).toHaveText("Working…");
  // no turn reads "Stopped": the first one is still the agent's, working
  await expect(
    conversation(page).locator('[data-slot="turn-summary-text"]').filter({ hasText: "Stopped" }),
  ).toHaveCount(0);

  // the agent finishes its turn, then reads the message: the next job answers it
  await release(page);
  await expect(
    conversation(page).getByText("echo: echo you were wrong since line 1"),
  ).toBeVisible();
  await expect(badge(page)).toHaveText("Done");
  await expect(
    conversation(page).getByText("gate refactor the parser", { exact: true }),
  ).toHaveCount(1);
  await expectNoHorizontalScroll(page);

  // a page opened now reads the same note from the log
  await page.reload();
  await expect(notes(page)).toHaveText("Sent while Coder was working · read at its next step");
  await expect(
    conversation(page).getByText("gate refactor the parser", { exact: true }),
  ).toHaveCount(1);
  await expect(
    conversation(page).getByText("echo you were wrong since line 1", { exact: true }),
  ).toHaveCount(1);
});

test("an agent whose card does not list steer/v1 says the message is read after its turn", async ({
  page,
}) => {
  await working(page, "gate refactor the parser", "Reviewer");
  await box(page).fill("echo one more thing");
  await expect(send(page)).toHaveAccessibleDescription("Reviewer reads it after this turn");
  await send(page).click();
  await expect(notes(page)).toHaveText("Sent while Reviewer was working · read after this turn");
  await release(page);
  await expect(badge(page)).toHaveText("Done");
});

test("Stop and send, from the menu with the keyboard: the agent stops and the message starts again", async ({
  page,
}) => {
  const bodies = posted(page);
  await working(page, "slow refactor the parser");
  await box(page).fill("echo do X instead");
  // the menu is the second button of Send: Enter opens it, the arrows move, Enter chooses
  await more(page).focus();
  await page.keyboard.press("Enter");
  const menu = page.getByRole("menu");
  await expect(menu).toBeVisible();
  const items = menu.getByRole("menuitem");
  await expect(items).toHaveCount(2);
  await expect(items.nth(0)).toContainText("Send");
  await expect(items.nth(0)).toContainText("Coder reads it at its next step");
  await expect(items.nth(1)).toContainText("Stop and send");
  await expect(items.nth(1)).toContainText("Stops Coder and starts again with your message");
  await expect(items.nth(1)).toContainText("Ctrl/⌘ Shift Enter");
  await expect(items.nth(0)).toBeFocused();
  await page.keyboard.press("ArrowDown");
  await expect(items.nth(1)).toBeFocused();
  await page.keyboard.press("Enter");

  await expect(menu).toBeHidden();
  await expect(notes(page)).toHaveText("Stopped Coder · it starts again from here");
  expect(bodies[1]?.forwardedProps["vymalo.send"]).toBe("interrupt");
  // the abandoned job was never judged: the thread goes on to the next one and ends Done
  await expect(conversation(page).getByText("echo: echo do X instead")).toBeVisible();
  await expect(badge(page)).toHaveText("Done");
  await expect(
    conversation(page).getByText("slow refactor the parser", { exact: true }),
  ).toHaveCount(1);
  // the focus is where the person writes, not on a button that went away with the text
  await expect(box(page)).toBeFocused();
});

test("the keys: Enter sends, Ctrl or Cmd with Shift stops and sends, Shift+Enter is a new line", async ({
  page,
}) => {
  const bodies = posted(page);
  await working(page, "slow refactor the parser");
  await box(page).fill("first line");
  await box(page).press("Shift+Enter");
  await expect(box(page)).toHaveValue("first line\n");
  await box(page).pressSequentially("second line");
  expect(bodies).toHaveLength(1); // nothing was sent
  await box(page).press("ControlOrMeta+Shift+Enter");
  await expect(notes(page)).toHaveText("Stopped Coder · it starts again from here");
  expect(bodies[1]?.messages).toMatchObject([{ content: "first line\nsecond line" }]);
  expect(bodies[1]?.forwardedProps["vymalo.send"]).toBe("interrupt");
  await expect(badge(page)).toHaveText("Done");
});

test("Enter while the agent works is Send", async ({ page }) => {
  const bodies = posted(page);
  await working(page, "gate refactor the parser");
  await box(page).fill("echo one more thing");
  await box(page).press("Enter");
  await expect(notes(page)).toHaveText("Sent while Coder was working · read at its next step");
  expect(bodies[1]?.forwardedProps["vymalo.send"]).toBe("steer");
  await release(page);
  await expect(badge(page)).toHaveText("Done");
});

test("an idle thread takes a message the ordinary way: no menu, no note, no vymalo.send", async ({
  page,
}) => {
  const bodies = posted(page);
  await startThread(page, "echo hi");
  await expect(badge(page)).toHaveText("Done");
  await box(page).fill("echo and again");
  await expect(more(page)).toHaveCount(0);
  await send(page).click();
  await expect(conversation(page).getByText("echo: echo and again")).toBeVisible();
  expect(bodies).toHaveLength(2);
  expect(bodies[1]?.forwardedProps).not.toHaveProperty("vymalo.send");
  await expect(notes(page)).toHaveCount(0);
});

for (const scheme of ["light", "dark"] as const) {
  test.describe(`accessibility (${scheme})`, () => {
    // a turn fades in for 160 ms: axe would read colours that are not at rest yet
    test.use({ colorScheme: scheme, contextOptions: { reducedMotion: "reduce" } });

    test("axe: the running composer, its menu and the notes have no serious violations", async ({
      page,
    }) => {
      await working(page, "gate refactor the parser");
      await box(page).fill("echo you were wrong since line 1");
      await expect(send(page)).toBeEnabled();
      expect(await axeViolations(page)).toEqual([]);

      await more(page).click();
      await expect(page.getByRole("menu")).toBeVisible();
      expect(await axeViolations(page)).toEqual([]);
      await page.keyboard.press("Escape");
      await expect(page.getByRole("menu")).toBeHidden();
      await expect(box(page)).toBeFocused();

      await send(page).click();
      await expect(notes(page)).toHaveCount(1);
      expect(await axeViolations(page)).toEqual([]);
      await release(page);
      await expect(badge(page)).toHaveText("Done");
      expect(await axeViolations(page)).toEqual([]);
    });
  });
}
