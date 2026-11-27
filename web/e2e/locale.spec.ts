import { expect, type Page, test } from "@playwright/test";
import { badge, conversation, startThread, THREAD_URL } from "./helpers";

/*
 * The interface language in the composer (`locale/v1`, ADR 0085, docs/api/locale-v1.md), against the
 * mock: the tag of the page the person reads (`<html lang>`) rides the run that carries a message as
 * `forwardedProps["vymalo.locale"]`; the default page sends `en`; a page whose `lang` is not a tag
 * sends no member at all, rather than a 400 that would hold the message back; and a run that carries
 * no message (an answer to a surface) carries none.
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

test("the page's tag rides the run that carries a message", async ({ page }) => {
  await drawIn(page, "fr-FR");
  const bodies = posted(page);
  await page.goto("/");
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
