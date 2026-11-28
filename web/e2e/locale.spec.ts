import { expect, type Page, test } from "@playwright/test";
import {
  agentPicker,
  BASE_URL,
  badge,
  conversation,
  MOCK_URL,
  startThread,
  THREAD_URL,
} from "./helpers";

/*
 * The interface language in the composer (`locale/v1`, ADR 0085, docs/api/locale-v1.md), against the
 * mock: the tag of the page the person reads (`<html lang>`) rides the run that carries a message as
 * `forwardedProps["vymalo.locale"]`; the default page sends `en`; a page whose `lang` is not a tag
 * sends no member at all, rather than a 400 that would hold the message back; and a run that carries
 * no message (an answer to a surface) carries none.
 *
 * It also proves the choice: the account menu offers the languages the deployment names
 * (`ui.languages`), each by its own name; picking one puts it on `<html lang>` before the first paint
 * of the next load and on the next message, and the picker's home is that menu, not a page.
 *
 * It proves what this page puts on the wire, not what an agent does with it: that is the
 * orchestrator's and adam's side (dev/locale-e2e.sh, dev/locale-real-e2e.sh), and the mock echoes
 * nothing of the tag.
 */

const box = (page: Page) => page.getByLabel("Message", { exact: true });

/** The bodies of the runs the page posted, as they go out. */
function posted(page: Page) {
  const bodies: {
    messages: { content: string }[];
    forwardedProps: Record<string, unknown>;
  }[] = [];
  page.on("request", (r) => {
    if (r.method() === "POST" && /\/agui\/agents\//.test(r.url())) bodies.push(r.postDataJSON());
  });
  return bodies;
}

/** Draws the page in `lang` before the app runs: the tag is the page's, not the app's (option (a) is another card). */
async function drawIn(page: Page, lang: string) {
  await page.addInitScript((tag) => {
    const apply = () => {
      document.documentElement.lang = tag;
    };
    if (document.documentElement) apply();
    else document.addEventListener("DOMContentLoaded", apply, { once: true });
  }, lang);
}

async function send(page: Page, text: string) {
  await box(page).fill(text);
  await page.getByRole("button", { name: "Send" }).click();
}

/**
 * The account menu's Language group, opened if it is not (on a phone, from the thread list's sheet): the
 * interface language's home is that menu, beside Theme (web/README.md "The interface language").
 */
async function openLanguageGroup(page: Page, isMobile: boolean) {
  const group = page.getByRole("group", { name: "Language" });
  if (await group.isVisible()) return group;
  if (isMobile) {
    await page.getByRole("button", { name: "Threads", exact: true }).click();
    await expect(page.getByRole("dialog", { name: "Threads" })).toBeVisible();
  }
  await page.getByRole("button", { name: /^Account: / }).click();
  await expect(group).toBeVisible();
  return group;
}

/**
 * Opens the account menu and picks a language, leaving the menu open: a choice changes the page and nothing
 * navigates, so the person may go on (web/README.md "The interface language").
 */
async function chooseLanguage(page: Page, isMobile: boolean, name: string) {
  const group = await openLanguageGroup(page, isMobile);
  await group.getByRole("menuitemradio", { name }).click();
  return group;
}

/** Gets the page back to the composer: the menu closed, and on a phone the thread list's sheet. */
async function closeMenu(page: Page, isMobile: boolean) {
  await page.keyboard.press("Escape");
  if (isMobile) {
    // the sheet's own close is "Close"; "Close sidebar" is the desktop sidebar's
    await page
      .getByRole("dialog", { name: "Threads" })
      .getByRole("button", { name: "Close", exact: true })
      .click();
  }
}

/**
 * Records, from the first byte of the document, every change of `<html lang>` and the moment `<body>` is
 * inserted (the first thing that can paint), in order: the log of a page whose language was known before
 * its first paint is exactly `["lang:fr-FR", "body"]` and nothing after.
 */
async function recordLang(page: Page) {
  await page.addInitScript(() => {
    const events: string[] = [];
    (window as unknown as { __lang: { events: string[] } }).__lang = { events };
    new MutationObserver((records) => {
      for (const r of records) {
        if (r.type === "attributes") {
          events.push(`lang:${document.documentElement.getAttribute("lang")}`);
        } else if ([...r.addedNodes].some((n) => n.nodeName === "BODY")) {
          events.push("body");
        }
      }
    }).observe(document, {
      attributes: true,
      attributeFilter: ["lang"],
      childList: true,
      subtree: true,
    });
  });
}

const recorded = (page: Page) =>
  page.evaluate(() => (window as unknown as { __lang: { events: string[] } }).__lang);

test("the page's tag rides the run that carries a message", async ({ page }) => {
  await drawIn(page, "fr-FR");
  const bodies = posted(page);
  await page.goto("/");
  // the picker is a button only once an agent is chosen: Send before that says "Choose an agent first."
  await expect(agentPicker(page)).toBeVisible();
  await expect(box(page)).toBeVisible();
  // the fixture is what the assertion is about, and it is checked at the moment the run reads it:
  // the app hydrates between goto and the send, and this page's own <html lang="en"> must not win
  await expect(page.locator("html")).toHaveAttribute("lang", "fr-FR");
  await send(page, "echo bonjour");
  await expect(page).toHaveURL(THREAD_URL);
  expect(bodies[0]?.forwardedProps["vymalo.locale"]).toBe("fr-FR");
});

test("the default page sends en", async ({ page }) => {
  const bodies = posted(page);
  await page.goto("/");
  await expect(page.locator("html")).toHaveAttribute("lang", "en");
  await expect(agentPicker(page)).toBeVisible();
  await expect(box(page)).toBeVisible();
  await send(page, "echo hi");
  await expect(page).toHaveURL(THREAD_URL);
  // the app's own <html lang="en">: the tag is honest today, whatever the person's browser asks for
  expect(bodies[0]?.forwardedProps["vymalo.locale"]).toBe("en");
});

test("a lang that is not a tag sends no member at all", async ({ page }) => {
  await drawIn(page, "en_CM");
  const bodies = posted(page);
  await page.goto("/");
  await expect(page.locator("html")).toHaveAttribute("lang", "en_CM");
  await expect(agentPicker(page)).toBeVisible();
  await expect(box(page)).toBeVisible();
  await send(page, "echo hi");
  await expect(page).toHaveURL(THREAD_URL);
  // a hint is never a reason to hold a message back: nothing is sent rather than a tag the
  // orchestrator would refuse with a 400
  expect(bodies[0]?.forwardedProps).not.toHaveProperty("vymalo.locale");
});

test("an answer to a surface carries no member", async ({ page }) => {
  const bodies = posted(page);
  await startThread(page, "choices now", "Reviewer");
  await expect(badge(page)).toHaveText("Your turn");

  const ui = conversation(page).getByRole("region", { name: "Interface from reviewer" });
  await expect(ui.getByText("A few quick choices")).toBeVisible();
  const db = ui.getByRole("radiogroup", { name: "Which database?" });
  const login = ui.getByRole("radiogroup", { name: "Which login?" });
  const where = ui.getByRole("group", { name: /Where does it run\?/ });
  await db.getByText("Postgres", { exact: true }).click();
  await login.getByText("No login", { exact: true }).click();
  await where.getByText("Docker Compose", { exact: true }).click();
  await page.getByRole("button", { name: "Send answers" }).click();
  await expect(badge(page)).toHaveText("Done");

  // the message run carried the page's tag; the action that answered the surface is no message
  expect(bodies[0]?.forwardedProps["vymalo.locale"]).toBe("en");
  expect(bodies[1]?.messages).toEqual([]);
  expect(bodies[1]?.forwardedProps).not.toHaveProperty("vymalo.locale");
});

/*
 * The choice: the deployment names the languages (`ui.languages`), the account menu offers them, and the
 * person's pick is kept on this device under `chat.lang` and put on `<html lang>` by a head script before
 * the first paint (`src/lib/language.ts`; the page's policy allows the script by its hash, e2e/csp.spec.ts).
 */

test("the account menu offers the deployment's languages, each by its own name", async ({
  page,
  isMobile,
}) => {
  await page.goto("/");
  await expect(agentPicker(page)).toBeVisible();
  const group = await openLanguageGroup(page, isMobile);
  await expect(group.getByRole("menuitemradio")).toHaveText([
    "English (United Kingdom)",
    "Français (France)",
    "Schweizerdeutsch (Schweiz)",
    "Furlan (Italie)",
  ]);
  // nothing chosen yet: the page is drawn in the export's own tag (`en`), which is not one of the four
  await expect(group.getByRole("menuitemradio", { checked: true })).toHaveCount(0);
});

test("picking a language puts it on <html> at once and on the next message", async ({
  page,
  isMobile,
}) => {
  const bodies = posted(page);
  await page.goto("/");
  await expect(agentPicker(page)).toBeVisible();
  const group = await chooseLanguage(page, isMobile, "Français (France)");
  await expect(page.locator("html")).toHaveAttribute("lang", "fr-FR");
  expect(await page.evaluate(() => localStorage.getItem("chat.lang"))).toBe("fr-FR");
  await expect(group.getByRole("menuitemradio", { name: "Français (France)" })).toBeChecked();
  // the menu stays open after a choice, and nothing moved
  await expect(group).toBeVisible();
  await closeMenu(page, isMobile);

  await send(page, "echo bonjour");
  await expect(page).toHaveURL(THREAD_URL);
  // the send path is unchanged: it reads the page's tag when the run goes out
  expect(bodies[0]?.forwardedProps["vymalo.locale"]).toBe("fr-FR");
});

test("a chosen language survives a reload, set before the body exists", async ({
  page,
  isMobile,
}) => {
  await page.goto("/");
  await expect(agentPicker(page)).toBeVisible();
  await chooseLanguage(page, isMobile, "Français (France)");
  await expect(page.locator("html")).toHaveAttribute("lang", "fr-FR");

  await recordLang(page);
  await page.reload();
  await expect(agentPicker(page)).toBeVisible();
  const log = await recorded(page);
  // set once, before the body existed, and never corrected afterwards (by React, say)
  expect(log.events.slice(0, 2)).toEqual(["lang:fr-FR", "body"]);
  expect(log.events.filter((e) => e.startsWith("lang:"))).toEqual(["lang:fr-FR"]);
  await expect(page.locator("html")).toHaveAttribute("lang", "fr-FR");
});

test("a deployment that offers two languages shows two", async ({ page, isMobile }, info) => {
  const session = `e2e-locale-${info.workerIndex}-${info.testId}`;
  const res = await page.request.post(
    `${MOCK_URL}/__mock/config?languages=en-GB,fr-FR&session=${session}`,
  );
  expect(res.ok()).toBe(true);
  await page.context().addCookies([{ name: "mock-registry", value: session, url: BASE_URL }]);
  await page.goto("/");
  await expect(agentPicker(page)).toBeVisible();
  const group = await openLanguageGroup(page, isMobile);
  await expect(group.getByRole("menuitemradio")).toHaveText([
    "English (United Kingdom)",
    "Français (France)",
  ]);
});

test("a stored tag the client refuses leaves the page's own tag", async ({ page }) => {
  await page.addInitScript(() => localStorage.setItem("chat.lang", "en_CM"));
  await page.goto("/");
  await expect(agentPicker(page)).toBeVisible();
  // a hint is never a reason to hold a message back: a tag the orchestrator would refuse is no choice
  await expect(page.locator("html")).toHaveAttribute("lang", "en");
});
