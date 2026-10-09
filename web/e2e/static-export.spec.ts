import { expect, test } from "@playwright/test";
import { badge, conversation, openThreadList, startThread, THREAD_URL } from "./helpers";

/*
 * The web is a static export (ADR 0047, decision 1): one page per kind of address, `/threads/_` and `/s/_`, which the
 * static server answers for any thread or link (`Caddyfile`, `scripts/serve-static.ts`). The id comes from the address.
 * Moving between threads stays inside the page: the static server answers the data Next fetches for a thread
 * (`/threads/<id>.txt`) with the shell's, so no navigation loads the page again.
 */

test("a thread is opened by its address, and moving between threads in the sidebar never loads the page again", async ({
  page,
}, testInfo) => {
  // the mock is shared by the projects: titles of this run only
  const run = `${testInfo.project.name}-${Date.now().toString(36)}`;
  const firstText = `echo the first ${run}`;
  const secondText = `echo the second ${run}`;
  await startThread(page, firstText);
  await expect(badge(page)).toHaveText("Done");
  const first = page.url();
  await startThread(page, secondText);
  await expect(badge(page)).toHaveText("Done");
  const second = page.url();
  expect(first).toMatch(THREAD_URL);
  expect(second).not.toBe(first);

  // straight to an address: the shell, which reads the id
  await page.goto(first);
  await expect(conversation(page).getByText(firstText, { exact: true })).toBeVisible();

  // a mark on the page: a page that was loaded again has none
  await page.evaluate(() => {
    (window as unknown as { __kept: string }).__kept = "this page";
  });
  // on a phone the list is a sheet, opened first
  await openThreadList(page);
  await page
    .getByRole("navigation", { name: "Threads" })
    .getByRole("link", { name: secondText })
    .click();
  await expect(page).toHaveURL(second);
  await expect(conversation(page).getByText(secondText, { exact: true })).toBeVisible();
  await openThreadList(page);
  await page
    .getByRole("navigation", { name: "Threads" })
    .getByRole("link", { name: firstText })
    .click();
  await expect(page).toHaveURL(first);
  await expect(conversation(page).getByText(firstText, { exact: true })).toBeVisible();
  expect(await page.evaluate(() => (window as unknown as { __kept?: string }).__kept)).toBe(
    "this page",
  );
});

test("an address that is not a thread is the page that says so", async ({ page }) => {
  await page.goto("/threads/not-a-thread");
  await expect(page.getByRole("heading", { name: "Not found" })).toBeVisible();
  await page.goto("/no/such/page");
  await expect(page.getByRole("heading", { name: "Not found" })).toBeVisible();
});

test("the runtime configuration is read from the page's own origin, and an empty one is the web of the edge", async ({
  request,
}) => {
  const answer = await request.get("/config.json");
  expect(answer.status()).toBe(200);
  expect(answer.headers()["cache-control"]).toBe("no-cache");
  expect(await answer.json()).toEqual({});
});
