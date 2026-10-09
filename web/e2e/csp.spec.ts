import { expect, test } from "@playwright/test";
import { watchCsp } from "./csp";
import { badge, panelTab, panelToggle, startThread } from "./helpers";

/*
 * The content security policy of every page (ADR 0054, decision 10; for the static export, ADR 0047, Amendment
 * 2026-10-09, src/lib/csp.ts): the static server's header, and in every page a meta that allows exactly the hashes of
 * its inline scripts; and not one violation while the app is used (the scripts of Next, ours in the head, assistant-ui,
 * the files card, the panel). The mock is an edge deployment here: the policy is the same in both kinds.
 */

const sha256 = async (text: string) =>
  Buffer.from(await crypto.subtle.digest("SHA-256", new TextEncoder().encode(text))).toString(
    "base64",
  );

test("every page has the header half and a meta of its own scripts' hashes, and none lets a script or a frame in", async ({
  request,
}) => {
  for (const address of [
    "/",
    "/threads/0199c0de-0000-7000-8000-000000000001",
    "/s/a-token",
    "/auth/callback",
    "/auth/sign-out",
    "/signed-in",
    "/no-such-page",
  ]) {
    const answer = await request.get(address);
    const header = answer.headers()["content-security-policy"] ?? "";
    expect(header, address).toContain("default-src 'self'");
    expect(header, address).toContain("script-src 'self' 'unsafe-inline'");
    expect(header, address).toContain("connect-src 'self'");
    expect(header, address).toContain("frame-ancestors 'none'");
    expect(header, address).toContain("object-src 'none'");
    expect(header, address).toContain("base-uri 'none'");
    expect(header, address).not.toContain("unsafe-eval");
    expect(answer.headers()["x-content-type-options"]).toBe("nosniff");
    expect(answer.headers()["referrer-policy"]).toBe("same-origin");
    const html = await answer.text();
    // the meta comes first in <head>, before any script it governs
    const meta = /<head[^>]*><meta http-equiv="Content-Security-Policy" content="([^"]+)">/.exec(
      html,
    );
    expect(meta, address).not.toBeNull();
    const scriptSrc = (meta?.[1] ?? "").split("; ").find((d) => d.startsWith("script-src ")) ?? "";
    const allowed = scriptSrc.split(" ").slice(1);
    expect(allowed[0]).toBe("'self'");
    expect(allowed).not.toContain("'unsafe-inline'");
    const inline = [...html.matchAll(/<script\b([^>]*)>([\s\S]*?)<\/script>/g)]
      .filter((m) => !/\ssrc=/.test(m[1] ?? ""))
      .map((m) => m[2] ?? "");
    expect(inline.length, address).toBeGreaterThan(0);
    for (const body of inline) expect(allowed, address).toContain(`'sha256-${await sha256(body)}'`);
  }
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
