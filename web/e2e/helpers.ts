import { expect, type Page } from "@playwright/test";

export const THREAD_URL = /\/threads\/[0-9a-f-]{36}$/;

/** Start a thread from the home page with the given agent (default: the first, Coder). */
export async function startThread(page: Page, text: string, agent?: string) {
  await page.goto("/");
  const select = page.getByLabel("Agent");
  await expect(select).toBeVisible();
  if (agent) await select.selectOption({ label: agent });
  await page.getByLabel("Message").fill(text);
  await page.getByRole("button", { name: "Send" }).click();
  await expect(page).toHaveURL(THREAD_URL);
}

export const badge = (page: Page) => page.getByRole("status", { name: /^Thread state:/ });
