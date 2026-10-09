import AxeBuilder from "@axe-core/playwright";
import { test as base, expect, type Locator, type Page } from "@playwright/test";
import { uuidv7 } from "../src/lib/uuid";
import {
  animationsDone,
  badge,
  conversation,
  expectNoHorizontalScroll,
  MOCK_URL,
  openThreadList,
  showActivity,
  startThread,
  threadList,
} from "./helpers";

/*
 * Sharing a thread by a link (ADR 0040) against the mock, in a real browser: the owner's menu, dialog,
 * chip and sidebar mark; the page of a link for a signed-in reader, for anybody and for a link that
 * does not work. A mock session is a person (`me`), the deployment's cap on sharing (`sharing`) and
 * whether there is an identity (`signedIn`); it is named by a cookie, so each test is its own person
 * and its own deployment. The owner of an API-made thread is the default person, dev@example.com.
 */

const ORIGIN = "http://127.0.0.1:3000";

type Profile = "user" | "admin" | "read-only";
type Settings = { me?: Profile; sharing?: "disabled" | "internal" | "public"; signedIn?: boolean };

let sessions = 0;
/** A mock session with these settings, for a person of its own; returns its name. */
async function makeSession(settings: Settings = {}): Promise<string> {
  const session = `e2e-share-${process.pid}-${Date.now()}-${++sessions}`;
  const query = new URLSearchParams({ session });
  if (settings.me) query.set("me", settings.me);
  if (settings.sharing) query.set("sharing", settings.sharing);
  if (settings.signedIn !== undefined) query.set("signedIn", String(settings.signedIn));
  const res = await fetch(`${MOCK_URL}/__mock/config?${query}`, { method: "POST" });
  expect(res.ok).toBe(true);
  return session;
}

const test = base.extend<{
  /** Makes the page's browser a session with these settings; returns its cookie, for the test's own calls. */
  join: (settings?: Settings) => Promise<string>;
}>({
  join: async ({ context }, use) => {
    await use(async (settings = {}) => {
      const session = await makeSession(settings);
      await context.clearCookies({ name: "mock-registry" });
      await context.addCookies([{ name: "mock-registry", value: session, url: ORIGIN }]);
      return `mock-registry=${session}`;
    });
  },
});

/** A thread of the person `cookie` is, run to its end. */
async function threadOf(cookie: string, text: string, agent = "adam"): Promise<string> {
  const id = uuidv7();
  const res = await fetch(`${MOCK_URL}/agui/agents/${agent}`, {
    method: "POST",
    headers: { "Content-Type": "application/json", Accept: "text/event-stream", cookie },
    body: JSON.stringify({
      threadId: id,
      runId: "run-1",
      messages: [{ id: "m-1", role: "user", content: text }],
    }),
  });
  expect(res.ok).toBe(true);
  await res.text();
  return id;
}

/** The owner shares a thread, as the dialog does; returns the token of the link. */
async function shareAs(cookie: string, id: string, visibility: "internal" | "public") {
  const res = await fetch(`${MOCK_URL}/api/threads/${id}/share`, {
    method: "PUT",
    headers: { "Content-Type": "application/json", cookie },
    body: JSON.stringify({ visibility }),
  });
  expect(res.status).toBe(200);
  return ((await res.json()) as { url: string }).url.replace("/s/", "");
}

const dialog = (page: Page) => page.getByRole("dialog", { name: "Share this conversation" });
const chip = (page: Page) => page.locator('[data-slot="share-chip"]');
const radio = (page: Page, name: string) => dialog(page).getByRole("radio", { name });
const linkField = (page: Page) => dialog(page).getByLabel("Link", { exact: true });
const banner = (page: Page) => page.locator('[data-slot="shared-banner"]');
const NOT_WORKING = (page: Page) =>
  page.getByRole("heading", { level: 1, name: "This link does not work" });

/**
 * An arrow key held until the focus has moved: the radios select the one the focus arrives on only
 * while the key is down, which a key pressed and released at once (a script's) never lets them see.
 */
async function arrow(page: Page, key: "ArrowDown" | "ArrowUp", to: Locator) {
  await page.keyboard.down(key);
  await expect(to).toBeFocused();
  await page.keyboard.up(key);
}

/** Picks a choice of the dialog and saves it. */
async function choose(page: Page, name: string) {
  await radio(page, name).click();
  await dialog(page).getByRole("button", { name: "Save" }).click();
}

async function openShareDialog(page: Page) {
  await page.getByRole("button", { name: "Thread options" }).click();
  await page.getByRole("menuitem", { name: /Share…/ }).click();
  await expect(dialog(page)).toBeVisible();
}

async function axeViolations(page: Page) {
  await animationsDone(page);
  const results = await new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa"]).analyze();
  return results.violations.filter((v) => v.impact === "serious" || v.impact === "critical");
}

test.describe("the owner shares a thread", () => {
  test("as signed-in people: the link, the chip in the top bar, the mark in the sidebar, then Private", async ({
    page,
    context,
    join,
    isMobile,
  }) => {
    await context.grantPermissions(["clipboard-read", "clipboard-write"], { origin: ORIGIN });
    await join({ sharing: "public" });
    const words = `echo share ${uuidv7().slice(-12)}`;
    await startThread(page, words);
    await expect(badge(page)).toHaveText("Done");
    await expect(chip(page)).toHaveCount(0);

    await openShareDialog(page);
    // a dialog: the focus is inside it, and the choice made is the thread's, nothing to save
    await expect(dialog(page)).toContainText("People with the link read it as it goes on.");
    expect(await dialog(page).evaluate((d) => d.contains(document.activeElement))).toBe(true);
    await expect(radio(page, "Private")).toBeChecked();
    await expect(linkField(page)).toHaveCount(0);

    // picking is not sharing: the choice is the thread's when it is saved
    await radio(page, "Signed-in people with the link").click();
    await expect(chip(page)).toHaveCount(0);
    await dialog(page).getByRole("button", { name: "Save" }).click();
    await expect(linkField(page)).toHaveValue(new RegExp(`^${ORIGIN}/s/[A-Za-z0-9_-]{43}$`));
    await expect(chip(page)).toHaveText("Shared · signed-in");
    // who can read the thread is words in the bar, and an icon where a phone's bar has no room for them
    const chipWidth = (await chip(page).boundingBox())?.width ?? 0;
    if (isMobile) expect(chipWidth).toBeLessThan(36);
    else expect(chipWidth).toBeGreaterThan(100);
    const link = await linkField(page).inputValue();

    // Copy puts exactly that link on the clipboard, and says so
    await dialog(page).getByRole("button", { name: "Copy" }).click();
    await expect(dialog(page).getByRole("button", { name: "Copied" })).toBeVisible();
    expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(link);
    // Copy is an icon named by its words; New link and Stop sharing, which take the link away, keep theirs
    expect(
      (await dialog(page).getByRole("button", { name: "Copied" }).boundingBox())?.width ?? 99,
    ).toBeLessThan(48);
    await expect(dialog(page).getByRole("button", { name: "New link" })).toHaveText("New link");
    await expect(dialog(page).getByRole("button", { name: "Stop sharing" })).toHaveText(
      "Stop sharing",
    );

    // the public choice carries its warning, in words, and its choice shows in the chip
    await expect(dialog(page)).toContainText(
      "Anyone with this link can read this conversation, including what you pasted in it. Your e-mail is not shown.",
    );
    await choose(page, "Anyone with the link");
    await expect(chip(page)).toHaveText("Shared · public");
    await expect(linkField(page)).toHaveValue(link); // widening keeps the link

    // the sidebar row is marked, with words for a screen reader
    await page.keyboard.press("Escape");
    await expect(dialog(page)).toBeHidden();
    await openThreadList(page);
    const row = threadList(page).getByRole("link", { name: new RegExp(words) });
    await expect(row.locator('[data-slot="share-mark"]')).toHaveText(/shared · public/);
    await page.keyboard.press("Escape");

    // Private is stopping: the chip and the mark go
    await openShareDialog(page);
    await choose(page, "Private");
    await expect(chip(page)).toHaveCount(0);
    await expect(linkField(page)).toHaveCount(0);
    await expectNoHorizontalScroll(page);
  });

  test("a choice above the deployment's cap is disabled, and says why", async ({ page, join }) => {
    await join({ sharing: "internal" });
    await startThread(page, `echo cap ${uuidv7().slice(-12)}`);
    await expect(badge(page)).toHaveText("Done");
    await openShareDialog(page);
    await expect(radio(page, "Anyone with the link")).toBeDisabled();
    await expect(radio(page, "Signed-in people with the link")).toBeEnabled();
    await expect(dialog(page)).toContainText("This deployment shares only with signed-in people.");
    await expect(radio(page, "Anyone with the link")).toHaveAccessibleDescription(
      /shares only with signed-in people/,
    );
  });

  test("the keyboard does all of it, and Escape gives the focus back to the menu's button", async ({
    page,
    join,
  }) => {
    await join({ sharing: "public" });
    await startThread(page, `echo keys ${uuidv7().slice(-12)}`);
    await expect(badge(page)).toHaveText("Done");
    const options = page.getByRole("button", { name: "Thread options" });
    await options.focus();
    await page.keyboard.press("Enter");
    await page.getByRole("menuitem", { name: /Share…/ }).focus();
    await page.keyboard.press("Enter");
    await expect(dialog(page)).toBeVisible();
    // the radios are one tab stop, moved by the arrows, which pick as they go: walking past "Anyone
    // with the link" shares nothing, the Save does
    await radio(page, "Private").focus();
    await arrow(page, "ArrowDown", radio(page, "Signed-in people with the link"));
    await expect(radio(page, "Signed-in people with the link")).toBeChecked();
    await arrow(page, "ArrowDown", radio(page, "Anyone with the link"));
    await expect(radio(page, "Anyone with the link")).toBeChecked();
    await arrow(page, "ArrowUp", radio(page, "Signed-in people with the link"));
    await expect(radio(page, "Signed-in people with the link")).toBeChecked();
    await expect(chip(page)).toHaveCount(0);
    await page.keyboard.press("Tab"); // Cancel
    await page.keyboard.press("Tab"); // Save
    await expect(dialog(page).getByRole("button", { name: "Save" })).toBeFocused();
    await page.keyboard.press("Enter");
    await expect(chip(page)).toHaveText("Shared · signed-in");
    // focus never leaves the dialog while it is open
    for (let i = 0; i < 12; i++) {
      await page.keyboard.press("Tab");
      expect(await dialog(page).evaluate((d) => d.contains(document.activeElement))).toBe(true);
    }
    // the tooltip of the control that has the focus goes first (WCAG 1.4.13: it can be dismissed without
    // moving the focus), and the dialog stays; on a control with none, one Escape is the dialog's
    await dialog(page).getByRole("button", { name: "Copy" }).focus();
    await expect(page.locator('[role="tooltip"]:not([data-state="closed"])')).toHaveText(
      "Copy the link",
    );
    await page.keyboard.press("Escape");
    await expect(page.locator('[role="tooltip"]')).toHaveCount(0);
    await expect(dialog(page)).toBeVisible();
    await dialog(page).getByRole("button", { name: "Done" }).focus();
    await page.keyboard.press("Escape");
    await expect(dialog(page)).toBeHidden();
    await expect(options).toBeFocused();
  });

  test("a new link replaces the old one, and Stop sharing takes it down", async ({
    page,
    browser,
    join,
  }) => {
    const cookie = await join({ sharing: "public" });
    const id = await threadOf(cookie, `echo links ${uuidv7().slice(-12)}`);
    const first = await shareAs(cookie, id, "public");
    await page.goto(`/threads/${id}`);
    await expect(badge(page)).toHaveText("Done");
    await expect(chip(page)).toHaveText("Shared · public");
    await openShareDialog(page);
    await expect(linkField(page)).toHaveValue(`${ORIGIN}/s/${first}`);

    await dialog(page).getByRole("button", { name: "New link" }).click();
    await expect(linkField(page)).not.toHaveValue(`${ORIGIN}/s/${first}`);
    await expect(dialog(page).getByRole("status")).toHaveText(
      "New link made. The old link no longer works.",
    );
    const second = (await linkField(page).inputValue()).replace(`${ORIGIN}/s/`, "");

    // a reader with no sign-in, in a browser of their own (a session of their own: the deployment's cap is the same)
    const reader = await browser.newContext({ baseURL: ORIGIN });
    const stranger = await makeSession({ sharing: "public", signedIn: false });
    await reader.addCookies([{ name: "mock-registry", value: stranger, url: ORIGIN }]);
    const there = await reader.newPage();
    await there.goto(`/s/${first}`);
    await expect(NOT_WORKING(there)).toBeVisible();
    await there.goto(`/s/${second}`);
    await expect(banner(there)).toBeVisible();

    await dialog(page).getByRole("button", { name: "Stop sharing" }).click();
    await expect(chip(page)).toHaveCount(0);
    await there.reload();
    await expect(NOT_WORKING(there)).toBeVisible();
    await reader.close();
  });

  test("a person whose roles may not share has no Share… for a private thread, and still can take a link down", async ({
    page,
    join,
  }) => {
    // a thread of the person who reads and does not write (`GET /api/me` says `sharing: disabled`
    // whatever the cap is, as they hold no `thread.share`): made by the default person and handed over
    const maker = await join({ me: "user", sharing: "public" });
    const id = await threadOf(maker, `echo off ${uuidv7().slice(-12)}`);
    const handed = await fetch(`${MOCK_URL}/__mock/owner?thread=${id}&owner=viewer@example.com`, {
      method: "POST",
    });
    expect(handed.status).toBe(204);
    await join({ me: "read-only", sharing: "public" });
    await page.goto(`/threads/${id}`);
    await expect(badge(page)).toHaveText("Done");
    await page.getByRole("button", { name: "Thread options" }).click();
    await expect(page.getByRole("menuitem", { name: "Export JSON" })).toBeVisible();
    await expect(page.getByRole("menuitem", { name: /Share…/ })).toHaveCount(0);
    await page.keyboard.press("Escape");

    // shared by an earlier state of the deployment: Share… is there again, for taking it down
    const shared = await fetch(`${MOCK_URL}/__mock/share?thread=${id}&visibility=internal`, {
      method: "POST",
    });
    expect(shared.status).toBe(200);
    await page.reload();
    await expect(chip(page)).toHaveText("Shared · signed-in");
    await openShareDialog(page);
    await expect(radio(page, "Signed-in people with the link")).toBeDisabled();
    await dialog(page).getByRole("button", { name: "Stop sharing" }).click();
    await expect(chip(page)).toHaveCount(0);
  });
});

test.describe("the page of a link", () => {
  test("a signed-in reader has the conversation and nothing that acts, and not the owner's address", async ({
    page,
    join,
  }) => {
    const owner = await join({ sharing: "public" });
    const words = `Fix the redirect loop ${uuidv7().slice(-8)}`;
    const id = await threadOf(owner, words);
    const token = await shareAs(owner, id, "internal");
    await join({ me: "admin", sharing: "public" });

    await page.goto(`/s/${token}`);
    await expect(banner(page)).toHaveText("Shared conversation, read only");
    await expect(badge(page)).toHaveText("Done", { timeout: 20_000 });
    await expect(conversation(page).getByText(words).first()).toBeVisible();
    await expect(page.getByRole("heading", { level: 1 })).toContainText("Fix the redirect loop");
    // no box, no menu, no fork, no edit, no rename, no export, no other agent
    for (const gone of [
      page.getByRole("textbox"),
      page.getByRole("button", { name: "Send" }),
      page.getByRole("button", { name: "Thread options" }),
      page.getByRole("button", { name: /^Agent:/ }),
      page.getByRole("button", { name: "Fork from here" }),
      page.getByRole("button", { name: "Edit what you said" }),
      page.getByRole("button", { name: "Stop" }),
    ]) {
      await expect(gone).toHaveCount(0);
    }
    await conversation(page).hover();
    await expect(page.getByRole("button", { name: "Fork from here" })).toHaveCount(0);
    // the owner's address is nowhere in the page
    expect(await page.content()).not.toContain("dev@example.com");
    // robots are told, by the page and by the API
    await expect(page.locator('meta[name="robots"]')).toHaveAttribute("content", /noindex/);
    // the steps are the panel's, read-only like the rest
    const activity = await showActivity(page);
    await expect(activity.locator('[data-slot="turn-section"]').first()).toBeVisible();
    await expectNoHorizontalScroll(page);
  });

  test("the files of the thread are read by the link's own route, signed in and not for anybody", async ({
    page,
    join,
  }) => {
    const owner = await join({ sharing: "public" });
    const id = await threadOf(owner, "files make some", "reviewer");
    const token = await shareAs(owner, id, "public");

    await join({ me: "admin", sharing: "public" });
    // a browser that has used the app is read by the signed-in route first (a browser that never has is read
    // by the public one, which names no files: ADR 0040, the amendment of 2026-10-07)
    await page.goto("/");
    await page.evaluate(() => window.localStorage.setItem("another-agentic.had-session", "1"));
    await page.goto(`/s/${token}`);
    await expect(banner(page)).toBeVisible();
    const card = conversation(page).locator('[data-slot="file-card"]').first();
    await expect(card).toBeVisible();
    const hrefs = await conversation(page)
      .locator('[data-slot="file-card"] a, [data-slot="file-card"] img')
      .evaluateAll((els) =>
        els.map((e) => (e as HTMLAnchorElement).href ?? (e as HTMLImageElement).src),
      );
    expect(hrefs.length).toBeGreaterThan(0);
    for (const href of hrefs) {
      expect(href).toMatch(new RegExp(`^${ORIGIN}/api/shared/${token}/artifacts/[0-9a-f]{64}`));
    }
    // and the file is there: the shared route answers it
    const first = hrefs[0] ?? "";
    expect((await page.request.get(first)).status()).toBe(200);

    // anybody: the stream does not name files, so there is no card
    await join({ me: "user", sharing: "public", signedIn: false });
    await page.evaluate(() => window.localStorage.clear());
    await page.goto(`/s/${token}`);
    await expect(banner(page)).toBeVisible();
    await expect(badge(page)).toHaveText("Done", { timeout: 20_000 });
    await expect(conversation(page).locator('[data-slot="file-card"]')).toHaveCount(0);
  });

  test("images in the answer that mean shared files: the link's own route for a signed-in reader, a placeholder for anybody", async ({
    page,
    join,
  }) => {
    const owner = await join({ sharing: "public" });
    const id = await threadOf(owner, "inline-images show me", "reviewer");
    const token = await shareAs(owner, id, "public");
    const words = conversation(page).locator('[data-slot="agent-message"]');

    await join({ me: "admin", sharing: "public" });
    await page.goto("/");
    await page.evaluate(() => window.localStorage.setItem("another-agentic.had-session", "1"));
    await page.goto(`/s/${token}`);
    await expect(banner(page)).toBeVisible();
    const list = words.getByRole("img", { name: "The list of people" });
    await expect(list).toBeVisible();
    expect(await list.getAttribute("src")).toMatch(
      new RegExp(`^/api/shared/${token}/artifacts/[0-9a-f]{64}$`),
    );
    await expect
      .poll(() => list.evaluate((el) => (el as HTMLImageElement).naturalWidth))
      .toBeGreaterThan(0);

    // anybody: the stream names no files and no step input, so every picture is its placeholder
    await join({ me: "user", sharing: "public", signedIn: false });
    await page.evaluate(() => window.localStorage.clear());
    await page.goto(`/s/${token}`);
    await expect(banner(page)).toBeVisible();
    await expect(badge(page)).toHaveText("Done", { timeout: 20_000 });
    const placeholders = words.locator('[data-slot="md-image-text"]');
    await expect(placeholders).toHaveCount(3);
    await expect(placeholders.nth(0)).toContainText("The list of people");
    await expect(placeholders.nth(1)).toContainText("Matches list with percentages");
    await expect(placeholders.nth(2)).toContainText("The login page");
    await expect(conversation(page).locator("img")).toHaveCount(0);
    await expectNoHorizontalScroll(page);
  });

  test("anybody reads a public link, with the public routes only: the signed-in one is never asked", async ({
    page,
    join,
  }) => {
    const owner = await join({ sharing: "public" });
    const id = await threadOf(owner, `echo for everybody ${uuidv7().slice(-8)}`);
    const token = await shareAs(owner, id, "public");
    await join({ sharing: "public", signedIn: false });
    const asked: string[] = [];
    page.on("response", (r) => {
      if (/\/(api|agui)\/(public\/)?shared\//.test(r.url())) {
        asked.push(`${new URL(r.url()).pathname.split(token)[0]}${r.status()}`);
      }
    });
    await page.goto(`/s/${token}`);
    await expect(banner(page)).toBeVisible();
    await expect(conversation(page).getByText("echo for everybody").first()).toBeVisible();
    // a browser that never had a session asks the public route first, and no route answers it 401
    expect(asked[0]).toBe("/api/public/shared/200");
    await expect.poll(() => asked).toContain("/agui/public/shared/200");
    expect(asked.filter((a) => a.endsWith("401"))).toEqual([]);
    expect(
      asked.filter((a) => a.startsWith("/api/shared/") || a.startsWith("/agui/shared/")),
    ).toEqual([]);
    expect(await page.content()).not.toContain("dev@example.com");
  });

  test("a link that does not work is one neutral page, whatever the reason", async ({
    page,
    join,
  }) => {
    const owner = await join({ sharing: "public" });
    const secret = `private words ${uuidv7().slice(-8)}`;
    const id = await threadOf(owner, `echo ${secret}`);
    const internal = await shareAs(owner, id, "internal");

    const pages: string[] = [];
    const seen = async (path: string) => {
      await page.goto(path);
      await expect(NOT_WORKING(page)).toBeVisible();
      await expect(page.getByText(secret)).toHaveCount(0);
      await expect(banner(page)).toHaveCount(0);
      await expect(page.getByRole("textbox")).toHaveCount(0);
      pages.push(await page.locator("main").innerText());
    };
    // a link nobody holds, one that cannot be a link, one for another kind of reader, a stopped one
    await join({ me: "admin", sharing: "public" });
    await seen(`/s/${"A".repeat(43)}`);
    await seen("/s/nope");
    await join({ sharing: "public", signedIn: false });
    await seen(`/s/${internal}`); // internal: not for anybody, and no sign-in is built into the image
    await join({ me: "admin", sharing: "public" });
    await fetch(`${MOCK_URL}/api/threads/${id}/share`, {
      method: "DELETE",
      headers: { cookie: owner },
    });
    await seen(`/s/${internal}`);
    // one page: the words never say which reason it was
    expect(new Set(pages).size).toBe(1);
    await expectNoHorizontalScroll(page);
  });

  test("a stream that has said all it has is let go, though the log ends in an event it has no frame for", async ({
    page,
    join,
  }) => {
    const owner = await join({ sharing: "public" });
    const id = await threadOf(owner, `echo quiet ${uuidv7().slice(-8)}`);
    const token = await shareAs(owner, id, "internal");
    await join({ me: "admin", sharing: "public" });
    // the thread is done and its log ends in `thread_shared`: the head the thread names is one the
    // stream never reaches, and the page must not hold the stream open for ever (a public one holds a permit)
    const ended = new Promise<string>((resolve) => {
      const route = `/agui/shared/${token}/connect`;
      page.on("requestfinished", (r) => r.url().includes(route) && resolve("finished"));
      page.on("requestfailed", (r) => r.url().includes(route) && resolve("aborted"));
    });
    await page.goto(`/s/${token}`);
    await expect(banner(page)).toBeVisible();
    await expect(badge(page)).toHaveText("Done");
    expect(["finished", "aborted"]).toContain(await ended);
  });
});

for (const scheme of ["light", "dark"] as const) {
  test.describe(`accessibility (${scheme})`, () => {
    test.use({ colorScheme: scheme, contextOptions: { reducedMotion: "reduce" } });

    test("axe: the share dialog, the page of a link and the page of a link that does not work have no serious violations", async ({
      page,
      join,
    }) => {
      const owner = await join({ sharing: "public" });
      const id = await threadOf(owner, `echo axe ${uuidv7().slice(-8)}`);
      const token = await shareAs(owner, id, "internal");

      await page.goto(`/threads/${id}`);
      await expect(badge(page)).toHaveText("Done");
      await openShareDialog(page);
      await expect(linkField(page)).toBeVisible();
      expect(await axeViolations(page)).toEqual([]);
      await page.keyboard.press("Escape");

      await join({ me: "admin", sharing: "public" });
      await page.goto(`/s/${token}`);
      await expect(banner(page)).toBeVisible();
      await expect(badge(page)).toHaveText("Done");
      expect(await axeViolations(page)).toEqual([]);

      await page.goto(`/s/${"A".repeat(43)}`);
      await expect(NOT_WORKING(page)).toBeVisible();
      expect(await axeViolations(page)).toEqual([]);
    });
  });
}
