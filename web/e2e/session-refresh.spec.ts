import { test as base, expect, type Page } from "@playwright/test";
import { uuidv7 } from "../src/lib/uuid";
import { agentPicker, MOCK_URL } from "./helpers";

/*
 * A session that is about to end, or has, without the page being lost (web/README.md "Signing in
 * again"), in a real browser against the mock. Run by `pnpm test:e2e:session` on its own build, the one
 * with the edge's sign-in built in (playwright.session.config.ts, port 3001). The mock plays the
 * edge's own routes (`/oauth2/userinfo` refreshes a token that has run out, `/oauth2/start` signs in);
 * a session of the mock is a person, and says `stale` (the token ran out: every call is a 401 until the
 * browser asks userinfo) or `signedIn=false` (no session at all). Every test types a draft and puts a
 * mark on the page's `window`: a page that was left, or reloaded, has neither.
 */

const ORIGIN = "http://127.0.0.1:3001";
const DRAFT = "a message the person has not sent yet";

let sessions = 0;
const test = base.extend<{
  /** The page's browser becomes a mock session (a person of its own); returns its name. */
  join: (settings?: string) => Promise<string>;
}>({
  join: async ({ context }, use) => {
    await use(async (settings = "") => {
      const session = `e2e-session-${process.pid}-${Date.now()}-${++sessions}`;
      const res = await fetch(`${MOCK_URL}/__mock/config?session=${session}&${settings}`, {
        method: "POST",
      });
      expect(res.ok).toBe(true);
      await context.clearCookies({ name: "mock-registry" });
      await context.addCookies([{ name: "mock-registry", value: session, url: ORIGIN }]);
      return session;
    });
  },
});

const config = (session: string, settings: string) =>
  fetch(`${MOCK_URL}/__mock/config?session=${session}&${settings}`, { method: "POST" });

const edge = async (session: string) =>
  (await (await fetch(`${MOCK_URL}/__mock/edge?session=${session}`)).json()) as {
    signedIn: boolean;
    stale: boolean;
    refreshes: number;
    signIns: number;
  };

const notice = (page: Page) => page.locator('[data-slot="session-banner"]');
const composer = (page: Page) => page.getByLabel("Message");

/** The new-chat page, a draft in the box and a mark on the window: what a page that was left would lose. */
async function openWithDraft(page: Page) {
  await page.goto("/");
  await expect(agentPicker(page)).toBeVisible();
  await composer(page).fill(DRAFT);
  await page.evaluate(() => {
    (window as unknown as { __kept: string }).__kept = "this page";
  });
}

async function expectKept(page: Page) {
  await expect(composer(page)).toHaveValue(DRAFT);
  expect(await page.evaluate(() => (window as unknown as { __kept?: string }).__kept)).toBe(
    "this page",
  );
}

/** The window comes back: the thread list is read again, which is a call that meets the 401. */
const windowComesBack = (page: Page) =>
  page.evaluate(() => window.dispatchEvent(new Event("focus")));

/** The statuses the page's own calls to the API were answered with, from here on. */
function answers(page: Page) {
  const seen: string[] = [];
  page.on("response", (r) => {
    const { pathname } = new URL(r.url());
    if (pathname.startsWith("/api/") || pathname.startsWith("/oauth2/")) {
      seen.push(`${pathname} ${r.status()}`);
    }
  });
  return seen;
}

test.describe("a session that can be refreshed", () => {
  test("is refreshed when a call meets the 401, and the page, its draft and its call go on", async ({
    page,
    join,
  }) => {
    const session = await join();
    await openWithDraft(page);
    const seen = answers(page);

    await config(session, "stale=true");
    await windowComesBack(page);

    // the call was refused, the browser asked the edge (which refreshed the token) and the call went again
    await expect
      .poll(() => seen)
      .toEqual(["/api/threads 401", "/oauth2/userinfo 200", "/api/threads 200"]);
    expect((await edge(session)).refreshes).toBe(1);
    await expect(notice(page)).toHaveCount(0);
    await expectKept(page);
    expect(new URL(page.url()).pathname).toBe("/");
  });

  test("is kept warm on an interval, so that the page never meets the 401", async ({
    page,
    join,
  }) => {
    const session = await join();
    await page.clock.install();
    await openWithDraft(page);
    const seen = answers(page);

    // the token runs out, as it does 15 minutes after a sign-in; the page is only open
    await config(session, "stale=true");
    await page.clock.fastForward("05:00");

    await expect.poll(async () => (await edge(session)).refreshes).toBe(1);
    await expect.poll(() => seen).toEqual(["/oauth2/userinfo 200"]);
    // and a call after it is not refused
    await windowComesBack(page);
    await expect.poll(() => seen).toContain("/api/threads 200");
    expect(seen.filter((s) => s.endsWith(" 401"))).toEqual([]);
    await expect(notice(page)).toHaveCount(0);
    await expectKept(page);
  });
});

test.describe("a session that has ended", () => {
  test("asks the person in a popup that closes itself, keeps the page and goes on when they are back", async ({
    page,
    join,
  }) => {
    const session = await join();
    await openWithDraft(page);
    const seen = answers(page);

    await config(session, "signedIn=false");
    await windowComesBack(page);
    await expect(notice(page)).toBeVisible();
    await expect(notice(page)).toContainText("Your session has ended");
    await expectKept(page);
    expect(new URL(page.url()).pathname).toBe("/");
    expect(seen.filter((s) => s.startsWith("/api/threads"))).toEqual(["/api/threads 401"]);

    // the sign-in opens in a window of its own: the issuer approves, and the last page closes it
    const [popup] = await Promise.all([
      page.waitForEvent("popup"),
      notice(page).getByRole("button", { name: "Sign in" }).click(),
    ]);
    await popup.waitForEvent("close");

    await expect(notice(page)).toHaveCount(0);
    // the call that was held went again, and was answered
    await expect.poll(() => seen).toContain("/api/threads 200");
    expect((await edge(session)).signIns).toBe(1);
    await expectKept(page);
    expect(new URL(page.url()).pathname).toBe("/");
  });

  test("leaves the page for the sign-in, and comes back to it, only when the popup is refused", async ({
    page,
    join,
  }) => {
    const session = await join();
    await page.addInitScript(() => {
      window.open = () => null;
    });
    await page.goto("/");
    await expect(agentPicker(page)).toBeVisible();

    await config(session, "signedIn=false");
    await windowComesBack(page);
    await expect(notice(page)).toBeVisible();
    // nothing left the page by itself: a person asked
    expect(new URL(page.url()).pathname).toBe("/");

    await notice(page).getByRole("button", { name: "Sign in" }).click();
    // the sign-in, then back to the page it came from (`rd`)
    await expect.poll(async () => (await edge(session)).signIns).toBe(1);
    await expect(agentPicker(page)).toBeVisible();
    await expect(notice(page)).toHaveCount(0);
    expect(new URL(page.url()).pathname).toBe("/");
  });
});

test.describe("a shared page", () => {
  test("a reader who is not signed in never meets the refresh, the banner or the sign-in", async ({
    page,
    join,
  }) => {
    // an owner shares a thread publicly
    const owner = await join("sharing=public");
    const id = uuidv7();
    const cookie = `mock-registry=${owner}`;
    const run = await fetch(`${MOCK_URL}/agui/agents/coder`, {
      method: "POST",
      headers: { "Content-Type": "application/json", Accept: "text/event-stream", cookie },
      body: JSON.stringify({
        threadId: id,
        runId: "run-1",
        messages: [{ id: "m-1", role: "user", content: "echo for everybody" }],
      }),
    });
    expect(run.ok).toBe(true);
    await run.text();
    const shared = await fetch(`${MOCK_URL}/api/threads/${id}/share`, {
      method: "PUT",
      headers: { "Content-Type": "application/json", cookie },
      body: JSON.stringify({ visibility: "public" }),
    });
    expect(shared.status).toBe(200);
    const token = ((await shared.json()) as { url: string }).url.replace("/s/", "");

    // anybody: no session at all
    await join("sharing=public&signedIn=false");
    const asked: string[] = [];
    page.on("request", (r) => asked.push(new URL(r.url()).pathname));
    await page.goto(`/s/${token}`);
    await expect(page.locator('[data-slot="shared-banner"]')).toBeVisible();
    await expect(page.getByText("echo for everybody").first()).toBeVisible();
    // the signed-in route said 401 and the public one answered: no question to the edge, no sign-in, no banner
    expect(asked.filter((p) => p.startsWith("/oauth2/"))).toEqual([]);
    await expect(notice(page)).toHaveCount(0);
    expect(new URL(page.url()).pathname).toBe(`/s/${token}`);
  });
});
