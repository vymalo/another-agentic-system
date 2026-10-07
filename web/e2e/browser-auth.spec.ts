import { test as base, expect, type Page } from "@playwright/test";
import { watchCsp } from "./csp";
import { agentPicker, badge, MOCK_URL, startThread } from "./helpers";

/*
 * The web that holds its own tokens (ADR 0054, web/README.md "Signing in again"), in a real browser
 * against the mock playing the issuer and checking every proof (mock/issuer.ts). Run by
 * `pnpm test:e2e:browser` on its own build and its own mock (playwright.browser-auth.config.ts, port
 * 3002). A mock session is a person and a deployment, named by the cookie `mock-registry`; the
 * issuer's counters are per session too, so every test reads its own.
 */

const ORIGIN = "http://127.0.0.1:3002";
const ISSUER = "http://127.0.0.1:4010";
const DB = "another-agentic-auth";

let sessions = 0;
const test = base.extend<{
  /** The page's browser becomes a mock session with these settings; returns the session's name. */
  join: (settings?: { issuer?: string; config?: string }) => Promise<string>;
}>({
  join: async ({ context }, use) => {
    await use(async (settings = {}) => {
      const session = `e2e-browser-${process.pid}-${Date.now()}-${++sessions}`;
      const res = await fetch(
        `${MOCK_URL}/__mock/issuer-config?session=${session}&${settings.issuer ?? ""}`,
        { method: "POST" },
      );
      expect(res.ok).toBe(true);
      if (settings.config) {
        await fetch(`${MOCK_URL}/__mock/config?session=${session}&${settings.config}`, {
          method: "POST",
        });
      }
      await context.clearCookies({ name: "mock-registry" });
      await context.addCookies([{ name: "mock-registry", value: session, url: ORIGIN }]);
      return session;
    });
  },
});

type Stats = {
  authorizations: number;
  codeGrants: number;
  refreshGrants: number;
  reuses: number;
  revocations: number;
  endSessions: number;
  apiRequests: number;
  publicWithCredentials: number;
  refused: Array<{ error: string; description: string }>;
};
const stats = async (session: string) =>
  (await (await fetch(`${MOCK_URL}/__mock/issuer?session=${session}`)).json()) as Stats;
const hook = (session: string, path: string) =>
  fetch(`${MOCK_URL}${path}${path.includes("?") ? "&" : "?"}session=${session}`, {
    method: "POST",
  });

/** What the app's origin holds in IndexedDB, read without creating anything. */
async function stored(page: Page) {
  return page.evaluate(async (name) => {
    const dbs = await indexedDB.databases();
    if (!dbs.some((d) => d.name === name)) return { exists: false as const };
    const db = await new Promise<IDBDatabase>((resolve, reject) => {
      const open = indexedDB.open(name);
      open.onsuccess = () => resolve(open.result);
      open.onerror = () => reject(open.error);
    });
    const count = (table: string) =>
      new Promise<number>((resolve, reject) => {
        const req = db.transaction(table).objectStore(table).count();
        req.onsuccess = () => resolve(req.result);
        req.onerror = () => reject(req.error);
      });
    const result = {
      exists: true as const,
      keys: await count("keys"),
      session: await count("session"),
      pending: await count("pending"),
    };
    db.close();
    return result;
  }, DB);
}

const notice = (page: Page) => page.locator('[data-slot="session-banner"]');
const composer = (page: Page) => page.getByLabel("Message");
/** The window comes back: the thread list is read again, a call that needs a good token. */
const windowComesBack = (page: Page) =>
  page.evaluate(() => window.dispatchEvent(new Event("focus")));

/** Signs in at the issuer (which approves at once) and waits for the app. */
async function open(page: Page, path = "/") {
  await page.goto(path);
  await expect(agentPicker(page)).toBeVisible({ timeout: 15_000 });
}

/** Every request to the orchestrator the page makes, with its credentials headers. */
function apiCalls(page: Page) {
  const calls: Array<{ path: string; authorization?: string; dpop?: string; status?: number }> = [];
  page.on("requestfinished", async (r) => {
    const { pathname } = new URL(r.url());
    if (!pathname.startsWith("/api/") && !pathname.startsWith("/agui/")) return;
    const headers = await r.allHeaders();
    const status = (await r.response())?.status();
    calls.push({
      path: pathname,
      ...(headers.authorization ? { authorization: headers.authorization } : {}),
      ...(headers.dpop ? { dpop: headers.dpop } : {}),
      ...(status ? { status } : {}),
    });
  });
  return calls;
}

test.describe("signing in", () => {
  test("goes to the issuer and back, loads the data with DPoP on every request, and breaks no policy", async ({
    page,
    join,
  }) => {
    const session = await join();
    const violations = await watchCsp(page);
    const calls = apiCalls(page);
    const issuerVisits: string[] = [];
    page.on("request", (r) => {
      if (r.url().startsWith(`${ISSUER}/oidc/auth`)) issuerVisits.push(new URL(r.url()).search);
    });

    await open(page, "/threads");
    await page.goto("/");
    await expect(agentPicker(page)).toBeVisible();

    // one visit to the issuer: PKCE S256, a state, the scope with offline_access, back to the callback
    expect(issuerVisits).toHaveLength(1);
    const query = new URLSearchParams(issuerVisits[0]);
    expect(query.get("code_challenge_method")).toBe("S256");
    expect(query.get("scope")).toContain("offline_access");
    expect(query.get("redirect_uri")).toBe(`${ORIGIN}/auth/callback`);

    // every call to the orchestrator carried the token and a proof, and the mock checked them
    const withoutAuth = calls.filter((c) => c.path !== "/api/public/auth");
    expect(withoutAuth.length).toBeGreaterThan(0);
    for (const call of withoutAuth) {
      expect(call.authorization, call.path).toMatch(/^DPoP /);
      expect(call.dpop, call.path).toMatch(/^[\w-]+\.[\w-]+\.[\w-]+$/);
    }
    const s = await stats(session);
    expect(s.refused).toEqual([]);
    expect(s.apiRequests).toBeGreaterThan(0);
    expect(s.codeGrants).toBe(1);

    // the address of the callback is not left in the history, and nothing of the sign-in is in storage
    expect(await page.evaluate(() => JSON.stringify({ ...localStorage }))).toBe("{}");
    expect(await page.evaluate(() => document.cookie)).not.toContain("refresh");
    expect(await violations()).toEqual([]);
  });

  test("keeps the person signed in across a reload: the session is in IndexedDB", async ({
    page,
    join,
  }) => {
    const session = await join();
    await open(page);
    const before = await stored(page);
    expect(before).toMatchObject({ exists: true, keys: 1, session: 1, pending: 0 });
    await page.reload();
    await expect(agentPicker(page)).toBeVisible();
    expect((await stats(session)).authorizations).toBe(1);
    expect((await stats(session)).codeGrants).toBe(1);
  });

  test("works when the browser's clock is an hour behind: the proofs are stamped with the server's time", async ({
    page,
    join,
  }) => {
    const session = await join();
    await page.clock.install({ time: Date.now() - 3_600_000 });
    await open(page);
    expect((await stats(session)).refused).toEqual([]);
    expect((await stats(session)).apiRequests).toBeGreaterThan(0);
  });
});

test.describe("a token that runs out", () => {
  test("is refreshed in silence when a request needs it, and the refresh token is rotated", async ({
    page,
    join,
  }) => {
    // a token with a minute and two seconds: under a minute left, two seconds on
    const session = await join({ issuer: "lifetime=62" });
    await open(page);
    const calls = apiCalls(page);
    await page.waitForTimeout(2500);
    await windowComesBack(page);
    await expect.poll(async () => (await stats(session)).refreshGrants).toBe(1);
    await expect
      .poll(() => calls.filter((c) => c.path === "/api/threads").length)
      .toBeGreaterThan(0);
    expect(calls.filter((c) => c.status === 401)).toEqual([]);
    await expect(notice(page)).toHaveCount(0);
    expect((await stats(session)).reuses).toBe(0);
  });
});

test.describe("a refresh token that is refused", () => {
  test("asks the person in a popup, keeps the page and its draft, and sends the held request when they are back", async ({
    page,
    join,
  }) => {
    const session = await join({ issuer: "lifetime=62" });
    await open(page);
    await composer(page).fill("a message not sent yet");
    await page.evaluate(() => {
      (window as unknown as { __kept: string }).__kept = "this page";
    });
    await page.waitForTimeout(2500);
    await hook(session, "/__mock/issuer-revoke");

    await windowComesBack(page);
    await expect(notice(page)).toBeVisible();
    await expect(notice(page)).toContainText("Your session has ended");

    const calls = apiCalls(page);
    const [popup] = await Promise.all([
      page.waitForEvent("popup"),
      notice(page).getByRole("button", { name: "Sign in" }).click(),
    ]);
    await popup.waitForEvent("close");
    await expect(notice(page)).toHaveCount(0);

    // the page is the one that was there, with its draft, and the held list was read after the sign-in
    await expect(composer(page)).toHaveValue("a message not sent yet");
    expect(await page.evaluate(() => (window as unknown as { __kept?: string }).__kept)).toBe(
      "this page",
    );
    await expect
      .poll(() => calls.some((c) => c.path === "/api/threads" && c.status === 200))
      .toBe(true);
    expect((await stats(session)).authorizations).toBe(2);
  });
});

test.describe("two tabs", () => {
  test("do not both redeem the refresh token: one refresh, no reuse, no banner", async ({
    page,
    context,
    join,
  }) => {
    // the issuer takes its time, so that both tabs would be in its grant together without the lock
    const session = await join({ issuer: "lifetime=62&refreshDelay=600" });
    await open(page);
    const other = await context.newPage();
    await open(other);
    await page.waitForTimeout(2500);
    await Promise.all([windowComesBack(page), windowComesBack(other)]);
    await expect.poll(async () => (await stats(session)).refreshGrants).toBeGreaterThanOrEqual(1);
    await page.waitForTimeout(1500);
    const s = await stats(session);
    expect(s.refreshGrants).toBe(1);
    expect(s.reuses).toBe(0);
    await expect(notice(page)).toHaveCount(0);
    await expect(notice(other)).toHaveCount(0);
  });
});

test.describe("a kept file", () => {
  test("is fetched with DPoP and drawn from a blob: no link to the route, no policy broken", async ({
    page,
    join,
  }) => {
    const session = await join();
    const violations = await watchCsp(page);
    await page.goto("/");
    await expect(agentPicker(page)).toBeVisible({ timeout: 15_000 });
    const calls = apiCalls(page);
    await startThread(page, "files make some", "Reviewer");
    await expect(badge(page)).toHaveText("Done");

    const img = page.locator('[data-slot="file-card"] [data-slot="file-image"]').first();
    await expect(img).toBeVisible();
    expect(await img.getAttribute("src")).toMatch(/^blob:http:\/\/127\.0\.0\.1:3002\//);
    await expect
      .poll(() =>
        img.evaluate(
          (el) => (el as HTMLImageElement).complete && (el as HTMLImageElement).naturalWidth,
        ),
      )
      .toBeGreaterThan(0);

    // no <img src> and no link of the page names the file's route
    expect(await page.locator('[src*="/artifacts/"], [href*="/artifacts/"]').count()).toBe(0);
    // the file was fetched like any other call: a token and a proof, and the mock took them
    const file = calls.filter((c) => /\/artifacts\/[0-9a-f]{64}/.test(c.path));
    expect(file.length).toBeGreaterThan(0);
    for (const call of file) {
      expect(call.authorization).toMatch(/^DPoP /);
      expect(call.dpop).toBeTruthy();
      expect(call.status).toBe(200);
    }
    expect((await stats(session)).refused).toEqual([]);
    expect(await violations()).toEqual([]);
  });
});

test.describe("signing out", () => {
  test("revokes the refresh token, ends the issuer's session and leaves nothing in IndexedDB", async ({
    page,
    join,
  }) => {
    const session = await join();
    const violations = await watchCsp(page);
    await open(page);
    expect(await stored(page)).toMatchObject({ exists: true, session: 1, keys: 1 });

    // the issuer's end-session page sends the browser back to the start page; the test stops it on
    // the sign-out page instead, which asks nothing, so that what is left can be read
    await page.route(`${ISSUER}/oidc/logout*`, async (route) => {
      const seen = await route.fetch({ maxRedirects: 0 });
      expect(seen.status()).toBe(302);
      await route.fulfill({
        status: 302,
        headers: { Location: `${ORIGIN}/auth/sign-out` },
      });
    });
    await page.goto("/auth/sign-out");
    await page.getByRole("button", { name: "Sign out" }).click();
    await expect(page).toHaveURL(`${ORIGIN}/auth/sign-out`);
    await expect.poll(async () => (await stats(session)).endSessions).toBe(1);

    const s = await stats(session);
    expect(s.revocations).toBe(1);
    expect(await stored(page)).toEqual({ exists: true, keys: 0, session: 0, pending: 0 });
    expect(await violations()).toEqual([]);
  });
});

test.describe("a public share link", () => {
  test("makes no token and no IndexedDB use for a reader who never signed in", async ({
    browser,
    page,
    join,
  }) => {
    const session = await join({ config: "sharing=public" });
    await open(page);
    // a thread of the signed-in person, shared by a public link
    await startThread(page, "echo a thread to share", "Reviewer");
    await expect(badge(page)).toHaveText("Done");
    const thread = /\/threads\/([0-9a-f-]{36})$/.exec(new URL(page.url()).pathname)?.[1] as string;
    const shared = await fetch(`${MOCK_URL}/__mock/share?thread=${thread}&visibility=public`, {
      method: "POST",
    });
    const { token } = (await shared.json()) as { token: string };

    // a browser that never signed in
    const reader = await browser.newContext();
    await reader.addCookies([{ name: "mock-registry", value: session, url: ORIGIN }]);
    const tab = await reader.newPage();
    const violations = await watchCsp(tab);
    const seen: Array<{ url: string; authorization?: string; dpop?: string }> = [];
    tab.on("request", async (r) => {
      const h = await r.allHeaders();
      seen.push({
        url: r.url(),
        ...(h.authorization ? { authorization: h.authorization } : {}),
        ...(h.dpop ? { dpop: h.dpop } : {}),
      });
    });
    await tab.goto(`/s/${token}`);
    await expect(tab.getByRole("log", { name: "Conversation" })).toBeVisible({ timeout: 15_000 });

    expect(seen.filter((r) => r.authorization || r.dpop)).toEqual([]);
    expect(seen.some((r) => r.url.startsWith(`${ISSUER}/oidc/`))).toBe(false);
    expect(
      seen.filter((r) => new URL(r.url).pathname.startsWith("/api/shared/")),
      "the signed-in route is not even tried without a stored session",
    ).toEqual([]);
    expect(
      await tab.evaluate(async () => (await indexedDB.databases()).map((d) => d.name)),
    ).toEqual([]);
    expect((await stats(session)).publicWithCredentials).toBe(0);
    expect(await violations()).toEqual([]);
    await reader.close();
  });
});
