import { expect, test } from "@playwright/test";
import { watchCsp } from "./csp";
import { badge, panelTab, panelToggle, startThread } from "./helpers";

/*
 * The content security policy of every page (ADR 0054, decision 10, src/proxy.ts): a nonce made for
 * each request, and not one violation while the app is used (the scripts of Next, ours in the head,
 * assistant-ui, the files card, the panel). The mock is an edge deployment here: the policy is the
 * same in both kinds.
 */

test("sets a policy with a new nonce on every page, and none lets a script or a frame in", async ({
  request,
}) => {
  const first = await request.get("/");
  const second = await request.get("/");
  const policy = first.headers()["content-security-policy"] ?? "";
  expect(policy).toContain("default-src 'self'");
  const scriptSrc = policy.split("; ").find((d) => d.startsWith("script-src ")) ?? "";
  expect(scriptSrc).toMatch(/^script-src 'self' 'nonce-[A-Za-z0-9+/=]+' 'strict-dynamic'$/);
  expect(policy).toContain("connect-src 'self'");
  expect(policy).toContain("frame-ancestors 'none'");
  expect(policy).toContain("object-src 'none'");
  expect(policy).toContain("base-uri 'none'");
  const nonce = (p: string) => /'nonce-([^']+)'/.exec(p)?.[1];
  expect(nonce(policy)).toBeTruthy();
  expect(nonce(second.headers()["content-security-policy"] ?? "")).not.toBe(nonce(policy));
  // the nonce is on the scripts of the page, ours in the head included
  const html = await first.text();
  const scripts = [...html.matchAll(/<script\b[^>]*>/g)].map((m) => m[0]);
  expect(scripts.length).toBeGreaterThan(0);
  for (const tag of scripts) expect(tag).toContain(`nonce="${nonce(policy)}"`);
});

test("reports no violation while a thread is started, followed and its panel read", async ({
  page,
}) => {
  const violations = await watchCsp(page);
  await startThread(page, "files make some", "Reviewer");
  await expect(badge(page)).toHaveText("Done");
  if ((await panelToggle(page).getAttribute("aria-expanded")) !== "true") {
    await panelToggle(page).click();
  }
  await panelTab(page, "Sources").click();
  await expect(page.locator('[data-slot="file-image"]').first()).toBeVisible();
  expect(await violations()).toEqual([]);
});

test("reports no violation on the sign-out page or the callback page", async ({ page }) => {
  const violations = await watchCsp(page);
  await page.goto("/auth/sign-out");
  await expect(page.getByRole("heading", { name: "Sign out" })).toBeVisible();
  await page.goto("/auth/callback?code=x&state=y");
  await expect(
    page.getByRole("heading", { name: /Signing in did not work|Signing you in/ }),
  ).toBeVisible();
  expect(await violations()).toEqual([]);
});
