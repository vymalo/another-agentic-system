import AxeBuilder from "@axe-core/playwright";
import { test as base, expect, type Page } from "@playwright/test";
import {
  animationsDone,
  badge,
  conversation,
  expectNoHorizontalScroll,
  MOCK_URL,
  openThreadList,
  THREAD_URL,
} from "./helpers";

/*
 * The access tier in the composer (ADR 0061, docs/api/access-tier-v1.md, web/README.md "Access tier"),
 * against the mock: the chip exists only while the selected agent's card lists the extension, offers
 * the tiers it honours up to the person's cap, shows the card's default and sends nothing until a tier
 * is picked, sends a pick with each message of the thread, remembers it per thread, forgets one the card
 * no longer honours and says so, and the bubble carries a badge for a message that was sent under one.
 * The mock lists the extension for no agent until a test says so for its own session.
 */

const ORIGIN = "http://127.0.0.1:3000";
const ALL = { tiers: ["plan", "edits", "auto", "bypass"], default: "auto" };

type Session = {
  /** What the card of `agent` lists under `access-tier/v1` from now on (`null`: nothing). */
  lists: (agent: string, params: unknown) => Promise<void>;
  /** Who the session is from now on (the page reads it when it loads). */
  become: (profile: "user" | "capped") => Promise<void>;
};

/** The mock keeps the cards and who the person is per session, named by a cookie: a test is its own. */
const test = base.extend<{ session: Session }>({
  session: async ({ context, request }, use, info) => {
    const name = `e2e-tier-${info.workerIndex}-${info.testId}-${info.repeatEachIndex}`;
    await context.addCookies([{ name: "mock-registry", value: name, url: ORIGIN }]);
    const hook = async (path: string, data?: unknown) => {
      const res = await request.post(
        `${MOCK_URL}/__mock/${path}${path.includes("?") ? "&" : "?"}session=${name}`,
        data === undefined ? {} : { data },
      );
      expect(res.ok()).toBe(true);
    };
    await use({
      lists: (agent, params) => hook("tiers", { agent, params }),
      become: (profile) => hook(`config?me=${profile}`),
    });
  },
});

const box = (page: Page) => page.getByLabel("Message", { exact: true });
const chip = (page: Page) => page.getByRole("button", { name: /^Access tier:/ });
const menu = (page: Page) => page.getByRole("menu");
const items = (page: Page) => menu(page).getByRole("menuitemradio");
const badges = (page: Page) => conversation(page).locator('[data-slot="tier-badge"]');

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

async function openMenu(page: Page) {
  await chip(page).click();
  await expect(menu(page)).toBeVisible();
}

/** Picks a tier of the open menu by its name; the menu closes. */
async function pick(page: Page, name: RegExp) {
  await openMenu(page);
  await items(page).filter({ hasText: name }).click();
  await expect(menu(page)).toBeHidden();
}

async function send(page: Page, text: string) {
  await box(page).fill(text);
  await page.getByRole("button", { name: "Send" }).click();
}

async function axeViolations(page: Page) {
  await animationsDone(page);
  const results = await new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa"]).analyze();
  return results.violations.filter((v) => v.impact === "serious" || v.impact === "critical");
}

test("an agent whose card lists nothing has no chip, and its messages carry no tier", async ({
  page,
}) => {
  const bodies = posted(page);
  await page.goto("/");
  await expect(box(page)).toBeVisible();
  await expect(chip(page)).toHaveCount(0);
  await send(page, "echo hi");
  await expect(page).toHaveURL(THREAD_URL);
  expect(bodies[0]?.forwardedProps).not.toHaveProperty("vymalo.tier");
  await expect(badge(page)).toHaveText("Done");
  await expect(badges(page)).toHaveCount(0);
});

test("a card that lists the extension badly has no chip either", async ({ page, session }) => {
  await session.lists("adam", { tiers: ["plan"], default: "auto" });
  await page.goto("/");
  await expect(box(page)).toBeVisible();
  await expect(chip(page)).toHaveCount(0);
});

test("the chip shows the card's default, offers the tiers with a line each, and sends nothing until one is picked", async ({
  page,
  session,
}) => {
  await session.lists("adam", ALL);
  const bodies = posted(page);
  await page.goto("/");
  await expect(chip(page)).toHaveAccessibleName("Access tier: Auto (the agent's default)");
  await openMenu(page);
  await expect(items(page)).toHaveCount(4);
  await expect(items(page)).toHaveText([
    /Plan\s*Read and propose/,
    /Edits\s*Change files, open PRs/,
    /Auto\s*default\s*Merge and create too/,
    /Bypass\s*Anything, never asks/,
  ]);
  await page.keyboard.press("Escape");
  await expect(menu(page)).toBeHidden();
  await send(page, "echo hi");
  await expect(page).toHaveURL(THREAD_URL);
  // the person picked nothing: the agent works under its default and the orchestrator is told nothing
  expect(bodies[0]?.forwardedProps).not.toHaveProperty("vymalo.tier");
  await expect(badges(page)).toHaveCount(0);
});

test("a pick goes with each message of the thread, the bubble wears a badge, and the thread remembers it", async ({
  page,
  session,
}) => {
  await session.lists("adam", ALL);
  const bodies = posted(page);
  await page.goto("/");
  await pick(page, /Plan/);
  await expect(chip(page)).toHaveAccessibleName("Access tier: Plan");
  await send(page, "echo look around");
  await expect(page).toHaveURL(THREAD_URL);
  await expect(badge(page)).toHaveText("Done");
  expect(bodies[0]?.forwardedProps["vymalo.tier"]).toBe("plan");
  await expect(badges(page)).toHaveText(["Sent under the access tier Plan"]);
  // the chip kept it: the thread's follow-up goes under it too
  await expect(chip(page)).toHaveAccessibleName("Access tier: Plan");
  await send(page, "echo and again");
  await expect(badges(page)).toHaveCount(2);
  expect(bodies[1]?.forwardedProps["vymalo.tier"]).toBe("plan");
  // a reload: the badges come from the log, the pick from this browser
  await page.reload();
  await expect(badges(page)).toHaveCount(2);
  await expect(chip(page)).toHaveAccessibleName("Access tier: Plan");
  // another tier for the next message only changes what that one carries
  await pick(page, /Edits/);
  await send(page, "echo change it");
  await expect(badges(page)).toHaveCount(3);
  expect(bodies[2]?.forwardedProps["vymalo.tier"]).toBe("edits");
  await expect(badges(page).last()).toContainText("Edits");
  await expectNoHorizontalScroll(page);
});

test("the pick of one thread is not the pick of another", async ({ page, session }) => {
  await session.lists("adam", ALL);
  await page.goto("/");
  await pick(page, /Edits/);
  await send(page, "echo first");
  await expect(page).toHaveURL(THREAD_URL);
  await expect(badge(page)).toHaveText("Done");
  // a new chat starts from the card's default again
  await openThreadList(page);
  await page.getByRole("link", { name: "New chat" }).first().click();
  await expect(chip(page)).toHaveAccessibleName("Access tier: Auto (the agent's default)");
});

test("the menu offers up to the cap of the person's role, and the chip shows what runs without a pick", async ({
  page,
  session,
}) => {
  // the cap is `edits` and the agent works under `auto` when none is named
  await session.lists("adam", ALL);
  await session.become("capped");
  const bodies = posted(page);
  await page.goto("/");
  await expect(chip(page)).toHaveAccessibleName("Access tier: Edits (the most your role allows)");
  await openMenu(page);
  await expect(items(page)).toHaveText([/Plan/, /Edits\s*default/]);
  await expect(items(page).filter({ hasText: /Auto|Bypass/ })).toHaveCount(0);
  await expect(menu(page)).toContainText(
    "Until you pick one, Adam works under Edits, the most your role allows.",
  );
  await page.keyboard.press("ArrowDown");
  await page.keyboard.press("Enter");
  await expect(menu(page)).toBeHidden();
  await send(page, "echo hi");
  await expect(page).toHaveURL(THREAD_URL);
  expect(bodies[0]?.forwardedProps["vymalo.tier"]).toBe("plan");
});

test("a capped person who picks nothing is run at their cap, never at the agent's default, and the bubble says so", async ({
  page,
  session,
}) => {
  await session.lists("adam", ALL);
  await session.become("capped");
  const bodies = posted(page);
  await page.goto("/");
  await send(page, "echo hi");
  await expect(page).toHaveURL(THREAD_URL);
  await expect(badge(page)).toHaveText("Done");
  // nothing was picked, so the web sent nothing; the orchestrator decided, and the log says what
  expect(bodies[0]?.forwardedProps).not.toHaveProperty("vymalo.tier");
  await expect(badges(page)).toHaveText(["Sent under the access tier Edits"]);
  await send(page, "echo and again");
  await expect(badge(page)).toHaveText("Done");
  expect(bodies[1]?.forwardedProps).not.toHaveProperty("vymalo.tier");
  // the orchestrator chose the tier of this one too, and the page hears of it when it reads the log
  await page.reload();
  await expect(badges(page)).toHaveText([
    "Sent under the access tier Edits",
    "Sent under the access tier Edits",
  ]);
  // the chip did not change: still what runs without a pick
  await expect(chip(page)).toHaveAccessibleName("Access tier: Edits (the most your role allows)");
});

test("an agent with no tier the person's role allows has no chip, and the send says why", async ({
  page,
  session,
}) => {
  await session.lists("adam", { tiers: ["auto", "bypass"], default: "auto" });
  await session.become("capped");
  await page.goto("/");
  await expect(box(page)).toBeVisible();
  await expect(chip(page)).toHaveCount(0);
  await box(page).fill("echo please");
  await page.getByRole("button", { name: "Send" }).click();
  await expect(
    page.getByRole("alert").filter({ hasText: "this agent has no tier your role allows" }),
  ).toBeVisible();
  await expect(box(page)).toHaveValue("echo please");
});

test("a pick the card no longer honours is forgotten, and the notice says so once", async ({
  page,
  session,
}) => {
  await session.lists("adam", ALL);
  await page.goto("/");
  await pick(page, /Bypass/);
  await send(page, "echo hi");
  await expect(page).toHaveURL(THREAD_URL);
  await expect(badge(page)).toHaveText("Done");
  // the agent stops listing bypass; the page reads the card again when it loads
  await session.lists("adam", { tiers: ["plan", "edits"], default: "edits" });
  const bodies = posted(page);
  await page.reload();
  const notice = page.locator('[data-slot="tier-notice"]');
  await expect(notice).toContainText("Your earlier choice, Bypass, is not offered any more");
  await expect(chip(page)).toHaveAccessibleName("Access tier: Edits (the agent's default)");
  await page.getByRole("button", { name: "Dismiss" }).click();
  await expect(notice).toHaveCount(0);
  // and the message goes out with no tier: the person is not silently put under another
  await send(page, "echo next");
  await expect(badge(page)).toHaveText("Done");
  expect(bodies.at(-1)?.forwardedProps).not.toHaveProperty("vymalo.tier");
  await page.reload();
  await expect(notice).toHaveCount(0);
});

test("a card that stops honouring the tier between the chip and the send is refused, and the text stays", async ({
  page,
  session,
}) => {
  await session.lists("adam", ALL);
  await page.goto("/");
  await pick(page, /Edits/);
  await box(page).fill("echo please");
  await session.lists("adam", { tiers: ["plan"], default: "plan" });
  await page.getByRole("button", { name: "Send" }).click();
  const alert = page.getByRole("alert").filter({ hasText: "does not take the tier 'edits'" });
  await expect(alert).toBeVisible();
  await expect(page).not.toHaveURL(THREAD_URL);
  await expect(box(page)).toHaveValue("echo please");
  await expect(badges(page)).toHaveCount(0);
});

test("a card that cannot be read is a 503 the person can retry, and the text stays", async ({
  page,
  session,
}) => {
  await session.lists("adam", ALL);
  await page.goto("/");
  await pick(page, /Plan/);
  await box(page).fill("echo please");
  await session.lists("adam", "unreadable");
  await page.getByRole("button", { name: "Send" }).click();
  await expect(page.getByRole("alert").filter({ hasText: "could not be read" })).toBeVisible();
  await expect(page).not.toHaveURL(THREAD_URL);
  await expect(box(page)).toHaveValue("echo please");
  // the card is back: the same message goes out under the same pick
  await session.lists("adam", ALL);
  await page.getByRole("button", { name: "Send" }).click();
  await expect(page).toHaveURL(THREAD_URL);
  await expect(badges(page)).toHaveText(["Sent under the access tier Plan"]);
});

test("capabilities that cannot be read when the page opens do not unsay the pick: the message goes with it", async ({
  page,
  session,
}) => {
  await session.lists("adam", ALL);
  await page.goto("/");
  await pick(page, /Plan/);
  await send(page, "echo first");
  await expect(page).toHaveURL(THREAD_URL);
  await expect(badge(page)).toHaveText("Done");
  // the page cannot read the agent's capabilities when it loads: no chip, but the remembered pick is
  // still the person's, and the orchestrator, which reads the card itself, decides
  await page.route("**/agui/agents/*/capabilities", (route) =>
    route.fulfill({ status: 503, contentType: "application/problem+json", body: "{}" }),
  );
  const bodies = posted(page);
  await page.reload();
  await expect(box(page)).toBeVisible();
  await expect(chip(page)).toHaveCount(0);
  await send(page, "echo more");
  await expect(badges(page)).toHaveCount(2);
  expect(bodies.at(-1)?.forwardedProps["vymalo.tier"]).toBe("plan");
  await expect(badges(page).last()).toContainText("Plan");
});

test("an edit goes out under the composer's pick, and the new chat keeps the pick", async ({
  page,
  session,
}) => {
  await session.lists("adam", ALL);
  const forks: Record<string, unknown>[] = [];
  page.on("request", (r) => {
    if (r.method() === "POST" && /\/api\/threads\/[^/]+\/fork$/.test(r.url())) {
      forks.push(r.postDataJSON());
    }
  });
  await page.goto("/");
  await pick(page, /Plan/);
  await send(page, "echo first");
  await expect(page).toHaveURL(THREAD_URL);
  await expect(badge(page)).toHaveText("Done");
  const original = page.url();
  const bubble = conversation(page).locator('[data-slot="user-message"]').first();
  await expect(bubble).toBeVisible();
  await bubble.hover();
  await bubble.getByRole("button", { name: "Edit what you said" }).click();
  const editor = page.getByRole("textbox", { name: "What you said" });
  await expect(editor).toBeFocused();
  await editor.fill("echo other");
  await editor.press("Control+Enter");
  await expect(page).not.toHaveURL(original);
  await expect(conversation(page).getByText("echo: echo other")).toBeVisible();
  expect(forks[0]).toMatchObject({ text: "echo other", tier: "plan" });
  // the edit is logged under it, and the chip of the new chat still has it
  await expect(badges(page).last()).toContainText("Plan");
  await expect(chip(page)).toHaveAccessibleName("Access tier: Plan");
});

test("bypass is marked by its icon and its words, on the chip, in the menu and on the bubble", async ({
  page,
  session,
}) => {
  await session.lists("adam", ALL);
  await page.goto("/");
  await openMenu(page);
  const bypass = items(page).filter({ hasText: "Bypass" });
  await expect(bypass).toContainText("Anything, never asks");
  await expect(bypass.locator("svg.lucide-triangle-alert")).toHaveCount(1);
  await expect(
    items(page).filter({ hasText: "Plan" }).locator("svg.lucide-triangle-alert"),
  ).toHaveCount(0);
  await bypass.click();
  await expect(chip(page)).toHaveAttribute("data-tier", "bypass");
  await expect(chip(page).locator("svg.lucide-triangle-alert")).toHaveCount(1);
  await send(page, "echo hi");
  await expect(page).toHaveURL(THREAD_URL);
  await expect(badges(page)).toHaveCount(1);
  await expect(badges(page).locator("svg.lucide-triangle-alert")).toHaveCount(1);
  await expect(badges(page)).toHaveAttribute("data-tier", "bypass");
});

test("the chip is reached and worked with the keyboard alone", async ({ page, session }) => {
  await session.lists("adam", ALL);
  const bodies = posted(page);
  await page.goto("/");
  await chip(page).focus();
  await page.keyboard.press("Enter");
  await expect(menu(page)).toBeVisible();
  await page.keyboard.press("ArrowDown");
  await page.keyboard.press("Enter");
  await expect(menu(page)).toBeHidden();
  // the focus is back on the chip, which says what was picked
  await expect(chip(page)).toBeFocused();
  await expect(chip(page)).toHaveAttribute("data-picked", "true");
  await box(page).focus();
  await page.keyboard.type("echo hi");
  await page.keyboard.press("Enter");
  await expect(page).toHaveURL(THREAD_URL);
  expect(bodies[0]?.forwardedProps["vymalo.tier"]).toMatch(/^(plan|edits|auto|bypass)$/);
});

for (const scheme of ["light", "dark"] as const) {
  test.describe(`accessibility (${scheme})`, () => {
    test.use({ colorScheme: scheme, contextOptions: { reducedMotion: "reduce" } });

    test("axe: the chip, its menu with bypass and a bubble with a badge have no serious violations", async ({
      page,
      session,
    }) => {
      await session.lists("adam", ALL);
      await page.goto("/");
      await expect(chip(page)).toBeVisible();
      expect(await axeViolations(page)).toEqual([]);
      await openMenu(page);
      expect(await axeViolations(page)).toEqual([]);
      await items(page).filter({ hasText: "Bypass" }).click();
      await expect(menu(page)).toBeHidden();
      expect(await axeViolations(page)).toEqual([]);
      await send(page, "echo hi");
      await expect(page).toHaveURL(THREAD_URL);
      await expect(badge(page)).toHaveText("Done");
      await expect(badges(page)).toHaveCount(1);
      expect(await axeViolations(page)).toEqual([]);
      await expectNoHorizontalScroll(page);
    });
  });
}
