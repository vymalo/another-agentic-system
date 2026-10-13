import AxeBuilder from "@axe-core/playwright";
import { expect, test } from "@playwright/test";
import {
  agentPicker,
  animationsDone,
  aPerson,
  chooseRowAction,
  expectNoHorizontalScroll,
  forkThread,
  makeThread,
  openRowMenu,
  openThreadList,
  type Person,
  rowMenuButton,
  rowOf,
  rowTitles,
  sectionTitles,
  threadList,
} from "./helpers";

/*
 * The person's own list (ADR 0042, web/DESIGN.md "Layout"): Pinned, Chats and Archived in the order the
 * server keeps, a fork nested under its parent, and a menu on each row (a vertical three-dot button
 * that shows on hover and on focus) to pin, archive and eject. Each test is a person of its own in the mock
 * (`aPerson`), so the list is theirs and its order can be asserted; the mock keeps the ranks as the
 * orchestrator does.
 */

/** Three finished chats of `person`, oldest first: `[a, b, c]`; the list has c on top. */
async function threeChats(person: Person, tag: string) {
  const titles = [`echo ${tag} alpha`, `echo ${tag} bravo`, `echo ${tag} charlie`] as const;
  const ids: string[] = [];
  for (const title of titles) ids.push(await makeThread(person, title));
  return { titles, ids };
}

async function listOpen(page: import("@playwright/test").Page) {
  await page.goto("/");
  await expect(agentPicker(page)).toBeVisible();
  await openThreadList(page);
  await expect(threadList(page)).toBeVisible();
}

test("the newest chat is on top, under Chats; there is no Today or Yesterday", async ({
  page,
}, info) => {
  const person = await aPerson(page, info);
  const { titles } = await threeChats(person, "order");
  await listOpen(page);
  const [a, b, c] = titles;
  await expect.poll(() => rowTitles(page)).toEqual([c, b, a]);
  await expect(threadList(page).getByRole("heading", { name: "Chats", exact: true })).toBeVisible();
  for (const old of ["Today", "Yesterday", "Previous 7 days", "Older"]) {
    await expect(threadList(page).getByRole("heading", { name: old })).toHaveCount(0);
  }
  // Pinned has nothing to show, so it is not drawn; Archived is a button that is closed
  await expect(threadList(page).getByRole("heading", { name: "Pinned" })).toHaveCount(0);
  await expect(threadList(page).getByRole("button", { name: "Archived" })).toHaveAttribute(
    "aria-expanded",
    "false",
  );
  await expectNoHorizontalScroll(page);
});

test("Pin moves a row to the top of Pinned and it stays there after a reload; Unpin puts it on top of the rest", async ({
  page,
}, info) => {
  const person = await aPerson(page, info);
  const { titles } = await threeChats(person, "pin");
  const [a, b, c] = titles;
  await listOpen(page);

  const request = page.waitForRequest(
    (r) => r.method() === "PATCH" && /\/api\/threads\/[^/]+\/rail$/.test(r.url()),
  );
  await chooseRowAction(page, a, "Pin");
  expect((await request).postDataJSON()).toEqual({ pinned: true });
  await expect.poll(() => sectionTitles(page, "Pinned")).toEqual([a]);
  await expect.poll(() => sectionTitles(page, "Chats")).toEqual([c, b]);
  // the menu of a pinned row says Unpin, and focus is back on its button
  await expect(rowMenuButton(page, a)).toBeFocused();

  await page.reload();
  await expect(agentPicker(page)).toBeVisible();
  await openThreadList(page);
  await expect.poll(() => sectionTitles(page, "Pinned")).toEqual([a]);
  await expect.poll(() => sectionTitles(page, "Chats")).toEqual([c, b]);

  // a second pin goes on top of the pinned
  await chooseRowAction(page, b, "Pin");
  await expect.poll(() => sectionTitles(page, "Pinned")).toEqual([b, a]);

  // unpin: on top of the rest, not back where its age put it
  await chooseRowAction(page, a, "Unpin");
  await expect.poll(() => sectionTitles(page, "Pinned")).toEqual([b]);
  await expect.poll(() => sectionTitles(page, "Chats")).toEqual([a, c]);
});

test("Archive moves a row to Archived, which is closed until it is opened; Unarchive brings it back where it was", async ({
  page,
}, info) => {
  const person = await aPerson(page, info);
  const { titles } = await threeChats(person, "archive");
  const [a, b, c] = titles;
  await listOpen(page);

  await chooseRowAction(page, b, "Archive");
  await expect.poll(() => sectionTitles(page, "Chats")).toEqual([c, a]);
  // the row is gone from where the person was: the focus goes to where it went, the Archived toggle
  await expect(threadList(page).getByRole("button", { name: "Archived" })).toBeFocused();

  // the archived are read when the section is opened, not before
  const toggle = threadList(page).getByRole("button", { name: "Archived" });
  await expect(toggle).toHaveAttribute("aria-expanded", "false");
  await expect(rowMenuButton(page, b)).toHaveCount(0);
  const reads = page.waitForRequest((r) => /\/api\/threads\?.*archived=only/.test(r.url()));
  await toggle.click();
  await reads;
  await expect(toggle).toHaveAttribute("aria-expanded", "true");
  await expect.poll(() => sectionTitles(page, "Archived")).toEqual([b]);

  // its menu offers Unarchive; the row goes back between the chats it was between
  await chooseRowAction(page, b, "Unarchive");
  await expect.poll(() => sectionTitles(page, "Chats")).toEqual([c, b, a]);
  await expect(threadList(page).getByText("Nothing archived.")).toBeVisible();

  // the archive is the person's, and survives a reload (the section is closed again)
  await chooseRowAction(page, a, "Archive");
  await page.reload();
  await expect(agentPicker(page)).toBeVisible();
  await openThreadList(page);
  await expect.poll(() => sectionTitles(page, "Chats")).toEqual([c, b]);
  await expect(threadList(page).getByRole("button", { name: "Archived" })).toHaveAttribute(
    "aria-expanded",
    "false",
  );
});

test("a fork is nested under its parent, and Eject takes it out, right after the block it left", async ({
  page,
}, info) => {
  const person = await aPerson(page, info);
  const { titles, ids } = await threeChats(person, "fork");
  const [a, b, c] = titles;
  const forkTitle = "echo fork child of bravo";
  const fork = await forkThread(person, ids[1] as string, forkTitle);
  await listOpen(page);

  // the fork is the row under "bravo", indented in the parent's item, and in the parent's section
  const parent = threadList(page)
    .locator('[data-slot="thread-item"]')
    .filter({ hasText: b })
    .first();
  const nested = parent.locator('[data-slot="thread-children"] a');
  await expect(nested).toHaveCount(1);
  await expect(nested).toHaveAttribute("href", `/threads/${fork}`);
  await expect.poll(() => rowTitles(page)).toEqual([c, b, forkTitle, a]);

  // a child can be archived alone or ejected, and cannot be pinned (the parent can)
  const parentMenu = await openRowMenu(page, b);
  await expect(parentMenu.getByRole("menuitem", { name: "Pin", exact: true })).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(page.getByRole("menu")).toBeHidden();
  const childMenu = await openRowMenu(page, forkTitle);
  await expect(childMenu.getByRole("menuitem")).toHaveText([
    "Eject from parent",
    "Archive",
    "Delete",
  ]);
  await childMenu.getByRole("menuitem", { name: "Eject from parent" }).click();
  await expect(page.getByRole("menu")).toBeHidden();

  // now a row of its own, right after its former block, and it can be pinned
  await expect(parent.locator('[data-slot="thread-children"]')).toHaveCount(0);
  await expect.poll(() => rowTitles(page)).toEqual([c, b, forkTitle, a]);
  const ownMenu = await openRowMenu(page, forkTitle);
  await expect(ownMenu.getByRole("menuitem", { name: "Pin", exact: true })).toBeVisible();
  await page.keyboard.press("Escape");
});

test("the menu button shows on hover and on keyboard focus, and is there for a touch screen", async ({
  page,
  isMobile,
}, info) => {
  const person = await aPerson(page, info);
  const { titles } = await threeChats(person, "reveal");
  await listOpen(page);
  const [a] = titles;
  const button = rowMenuButton(page, a);
  const opacity = () => button.evaluate((el) => getComputedStyle(el).opacity);
  if (isMobile) {
    // no hover on a touch screen: the button is always there
    expect(await opacity()).toBe("1");
    return;
  }
  expect(await opacity()).toBe("0");
  await rowOf(page, a).hover();
  await expect.poll(opacity).toBe("1");
  await page.mouse.move(0, 0);
  await expect.poll(opacity).toBe("0");
  // the keyboard: tabbing from the row's link to its button shows it
  await rowOf(page, a).getByRole("link").focus();
  await page.keyboard.press("Tab");
  await expect(button).toBeFocused();
  await expect.poll(opacity).toBe("1");
});

test("the keyboard opens a menu, chooses an item, and the focus comes back to the row's button", async ({
  page,
}, info) => {
  const person = await aPerson(page, info);
  const { titles } = await threeChats(person, "keys");
  const [a, b] = titles;
  await listOpen(page);
  const button = rowMenuButton(page, a);
  await button.focus();
  await page.keyboard.press("Enter");
  const menu = page.getByRole("menu");
  await expect(menu).toBeVisible();
  await expect(menu.getByRole("menuitem").first()).toBeFocused();
  // Escape closes it and the focus returns to the button that opened it
  await page.keyboard.press("Escape");
  await expect(menu).toBeHidden();
  await expect(button).toBeFocused();
  // Enter, the first item (Pin), Enter
  await page.keyboard.press("Enter");
  await expect(menu).toBeVisible();
  await page.keyboard.press("Enter");
  await expect(menu).toBeHidden();
  await expect.poll(() => sectionTitles(page, "Pinned")).toEqual([a]);
  expect(b).toBeTruthy();
});

test("a list that changed under the person (the chat is gone) is read again and says nothing worse", async ({
  page,
}, info) => {
  const person = await aPerson(page, info);
  const { titles, ids } = await threeChats(person, "gone");
  const [a, b, c] = titles;
  await listOpen(page);
  // another tab deleted "bravo"; this page still has its row
  const res = await page.request.delete(`/api/threads/${ids[1]}`);
  expect(res.status()).toBe(204);
  await chooseRowAction(page, b, "Pin");
  await expect.poll(() => rowTitles(page)).toEqual([c, a]);
  await expect(page.getByRole("alert").filter({ hasText: /Could not/ })).toHaveCount(0);
});

for (const scheme of ["light", "dark"] as const) {
  test.describe(`accessibility (${scheme})`, () => {
    test.use({ colorScheme: scheme, contextOptions: { reducedMotion: "reduce" } });

    async function serious(page: import("@playwright/test").Page, within?: string) {
      await animationsDone(page);
      const axe = new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa"]);
      if (within) axe.include(within);
      const results = await axe.analyze();
      return results.violations.filter((v) => v.impact === "serious" || v.impact === "critical");
    }

    test("axe: the sections, a nested fork, the open Archived and an open row menu have no serious violations", async ({
      page,
    }, info) => {
      const person = await aPerson(page, info);
      const { titles, ids } = await threeChats(person, "axe");
      await forkThread(person, ids[0] as string, "echo axe fork of alpha");
      const [a, b] = titles;
      await listOpen(page);
      await chooseRowAction(page, a, "Pin");
      await chooseRowAction(page, b, "Archive");
      await threadList(page).getByRole("button", { name: "Archived" }).click();
      await expect.poll(() => sectionTitles(page, "Archived")).toEqual([b]);
      expect(await serious(page)).toEqual([]);

      await openRowMenu(page, a);
      // (the menu alone: a modal Radix menu hides the page behind it from assistive technology while the page's
      // buttons stay focusable under it, which axe reads as `aria-hidden-focus`; see description.spec.ts)
      expect(await serious(page, '[role="menu"]')).toEqual([]);
    });
  });
}
