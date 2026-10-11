import { expect, type Locator, type Page, test } from "@playwright/test";
import { badge, conversation, expectNoHorizontalScroll, startThread } from "./helpers";

/**
 * Choices of version 5 against the mock orchestrator's `choices-deck` story: the coder shares two
 * screenshots, then asks which design to build with a deck of them (each option a picture of a file
 * of the thread, with a title and a subtitle, one recommended) and which ways to log in with a
 * searchable list of twelve, at most three, and a free box. One action answers both.
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

/** Every URL the page asked for. */
function requested(page: Page): string[] {
  const urls: string[] = [];
  page.on("request", (r) => urls.push(r.url()));
  return urls;
}

const FILE_ROUTE = /\/api\/threads\/[0-9a-f-]{36}\/artifacts\/[0-9a-f]{64}$/;

const send = (page: Page) => page.getByRole("button", { name: "Send answers" });
const deck = (page: Page) => page.getByRole("radiogroup", { name: "Which layout?" });
const tiles = (page: Page) => deck(page).locator('[data-slot="choices-deck-option"]');
const combo = (page: Page) => page.getByRole("combobox", { name: "Which ways to log in?" });
const chips = (page: Page) => page.locator('[data-slot="choices-chips"] li');

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

test.describe("Choices: an image deck and a searchable list", () => {
  test("the deck shows the thread's own pictures with a title and a subtitle; clicking a picture chooses its tile", async ({
    page,
  }) => {
    const urls = requested(page);
    await startThread(page, "choices-deck now", "Reviewer");
    await expect(badge(page)).toHaveText("Your turn");
    const ui = conversation(page).getByRole("region", { name: "Interface from reviewer" });
    await expect(ui.getByRole("heading", { name: "Which design should I build?" })).toBeVisible();
    await expect(ui.getByText("I made two screenshots of the people screen.")).toBeVisible();

    await expect(tiles(page)).toHaveCount(2);
    const list = tiles(page).nth(0);
    const matches = tiles(page).nth(1);
    const listPicture = list.getByRole("img", {
      name: "A list of four people with an avatar each",
    });
    const matchesPicture = matches.getByRole("img", {
      name: "The same four people with a bar that shows how well each fits",
    });
    await expectDecoded(listPicture);
    await expectDecoded(matchesPicture);
    // the pictures are the thread's files, from the API's own route, and nothing else is fetched
    for (const img of [listPicture, matchesPicture]) {
      expect(await img.getAttribute("src")).toMatch(
        /^\/api\/threads\/[0-9a-f-]{36}\/artifacts\/[0-9a-f]{64}$/,
      );
    }
    // nothing came from outside the app: no picture by a URL the agent wrote
    const origin = new URL(page.url()).origin;
    expect(urls.filter((u) => new URL(u).origin !== origin)).toEqual([]);
    expect(urls.filter((u) => u.includes("/artifacts/")).every((u) => FILE_ROUTE.test(u))).toBe(
      true,
    );

    await expect(list.getByText("A list", { exact: true })).toBeVisible();
    await expect(list.getByText("Four rows with an avatar each")).toBeVisible();
    await expect(matches.getByText("Recommended")).toBeVisible();

    // the whole tile is the target, the picture included (the label covers the tile, so the click
    // goes to it, as a person's does: `force` skips only Playwright's refusal to click under it)
    await matchesPicture.click({ force: true });
    await expect(deck(page).getByRole("radio", { name: "Matches" })).toBeChecked();
    await listPicture.click({ force: true });
    await expect(deck(page).getByRole("radio", { name: "A list" })).toBeChecked();
    await expect(deck(page).getByRole("radio", { name: "Matches" })).not.toBeChecked();
    await expectNoHorizontalScroll(page);
  });

  test("the list is searched by name and by subtitle, chosen from with the keys or the mouse, shows chips, and stops at three", async ({
    page,
    isMobile,
  }) => {
    await startThread(page, "choices-deck now", "Reviewer");
    await expect(badge(page)).toHaveText("Your turn");
    await expect(combo(page)).toBeVisible();
    await expect(page.getByRole("listbox")).toHaveCount(0);

    await combo(page).click();
    await expect(page.getByRole("option")).toHaveCount(12);
    await expect(page.getByText("Search by name")).toBeVisible();

    // the name is searched, and the subtitle too
    await combo(page).fill("pass");
    await expect(page.getByRole("option")).toHaveCount(2); // Password, Passkey
    await combo(page).fill("directory");
    await expect(page.getByRole("option")).toHaveCount(1); // LDAP: "A directory server"
    await combo(page).fill("nothing like this");
    await expect(page.getByText("No match")).toBeVisible();

    await combo(page).fill("pass");
    if (!isMobile) {
      // the arrows move, Enter chooses, and the list stays for the next one
      await page.keyboard.press("ArrowDown");
      await page.keyboard.press("Enter");
      await expect(chips(page)).toHaveText(["Passkey"]);
    } else {
      await page.getByRole("option", { name: /Passkey/ }).click();
      await expect(chips(page)).toHaveText(["Passkey"]);
    }
    await combo(page).fill("git");
    await page.getByRole("option", { name: /GitHub/ }).click();
    await combo(page).fill("goo");
    await page.getByRole("option", { name: /Google/ }).click();
    await expect(chips(page)).toHaveCount(3);
    // three is the most: the fourth cannot be taken
    await combo(page).fill("apple");
    await expect(page.getByRole("option", { name: /Apple/ })).toHaveAttribute(
      "aria-disabled",
      "true",
    );
    // (aria-disabled: Playwright would wait for it to be enabled, so the click is forced)
    await page.getByRole("option", { name: /Apple/ }).click({ force: true });
    await expect(chips(page)).toHaveCount(3);

    // a chip is given back with its button
    await page.getByRole("button", { name: "Remove Google" }).click();
    await expect(chips(page)).toHaveCount(2);
    await expectNoHorizontalScroll(page);
  });

  test("one action answers the deck, the list and the free box; the bubble shows the labels, and a reload replays it", async ({
    page,
  }) => {
    const posts = runPosts(page);
    await startThread(page, "choices-deck now", "Reviewer");
    await expect(badge(page)).toHaveText("Your turn");
    await expect(send(page)).toBeDisabled();

    await tiles(page).nth(1).getByRole("img").click({ force: true });
    await combo(page).click();
    await page.getByRole("option", { name: /Passkey/ }).click();
    await page.getByRole("option", { name: /GitHub/ }).click();
    await expect(send(page)).toBeEnabled();
    await page.getByLabel("Anything to change?").fill("Make the avatars round");
    await page.waitForTimeout(300);
    expect(posts).toHaveLength(1);

    await send(page).click();
    await expect(badge(page)).toHaveText("Done");
    expect(posts).toHaveLength(2);
    const action = posts[1] as Action;
    expect(action.messages).toEqual([]);
    expect(action.forwardedProps.a2uiAction?.userAction).toMatchObject({
      name: "answer",
      sourceComponentId: "pick",
      // in the order of the options, not of the clicks
      context: {
        answers: [
          { id: "layout", values: ["matches"] },
          { id: "login", values: ["passkey", "github"] },
        ],
        note: "Make the avatars round",
      },
    });
    await expect(
      conversation(page).getByText(
        "answered: ui-action answer layout=matches login=passkey,github note:Make the avatars round",
      ),
    ).toHaveCount(1);

    const bubble = page.locator('[data-slot="answer-bubble"]');
    await expect(bubble).toBeVisible();
    const lines = await page
      .locator('[data-slot="answer-line"]')
      .evaluateAll((els) =>
        els.map((l) => [
          l.querySelector("dt")?.textContent ?? "",
          l.querySelector("dd")?.textContent ?? "",
        ]),
      );
    expect(lines).toEqual([
      ["Which layout?", "Matches"],
      ["Which ways to log in?", "Passkey, GitHub"],
    ]);
    await expect(bubble.locator('[data-slot="answer-note"] dd')).toHaveText(
      "Make the avatars round",
    );

    // the form keeps its place, takes no more answers, and the deck is still the pictures
    await expect(send(page)).toBeDisabled();
    await expect(deck(page).getByRole("radio", { name: "Matches" })).toBeDisabled();
    await expect(combo(page)).toBeDisabled();
    await expectDecoded(tiles(page).nth(0).getByRole("img"));

    await page.reload();
    await expect(badge(page)).toHaveText("Done");
    await expect(page.locator('[data-slot="answer-bubble"]')).toHaveCount(1);
    await expect(send(page)).toBeDisabled();
    await expectNoHorizontalScroll(page);
  });
});
