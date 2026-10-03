import AxeBuilder from "@axe-core/playwright";
import { test as base, expect, type Page } from "@playwright/test";
import {
  animationsDone,
  badge,
  chooseAgent,
  conversation,
  expectNoHorizontalScroll,
  MOCK_URL,
  startThread,
  THREAD_URL,
} from "./helpers";

/*
 * Mentions in the composer (ADR 0026, web/DESIGN.md "Mentions"), against the mock: an "@" opens the
 * agents that may be mentioned, the keyboard alone picks one, the message goes out with the reference
 * and the bubble draws it, the warning above the box follows the addressed agent's card, and a
 * refused send keeps the text with its mentions. The mock lists mentions/v1 for the Coder and the
 * Verifier, thread-tools/v1 for the Coder only, and neither for the Reviewer.
 */

const ORIGIN = "http://127.0.0.1:3000";

type Session = {
  /** The platform lists this agent from now on. */
  add: (agent: { id: string; name: string }) => Promise<void>;
  /** The registry can (not) be read. */
  down: (down: boolean) => Promise<void>;
  /** Who the session is from now on (the page has already read who it is). */
  become: (profile: "user" | "limited") => Promise<void>;
};

/** The mock keeps the registry and who the person is per session, named by a cookie: a test is its own. */
const test = base.extend<{ session: Session }>({
  session: async ({ context, request }, use, info) => {
    const name = `e2e-${info.workerIndex}-${info.testId}`;
    await context.addCookies([{ name: "mock-registry", value: name, url: ORIGIN }]);
    const hook = async (path: string, data?: unknown) => {
      const res = await request.post(
        `${MOCK_URL}/__mock/${path}${path.includes("?") ? "&" : "?"}session=${name}`,
        data === undefined ? {} : { data },
      );
      expect(res.ok()).toBe(true);
    };
    await use({
      add: (agent) => hook("registry/agents", agent),
      down: (down) => hook(`registry?down=${down}`),
      become: (profile) => hook(`config?me=${profile}`),
    });
  },
});

const box = (page: Page) => page.getByLabel("Message");
const list = (page: Page) => page.getByRole("listbox", { name: "Agents to mention" });
const warning = (page: Page) => page.locator('[data-slot="mentions-warning"]');
const chips = (page: Page) =>
  page.getByRole("list", { name: "Mentioned agents" }).getByRole("listitem");
const bubbleChips = (page: Page) => conversation(page).locator('[data-slot="mention"]');

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

async function axeViolations(page: Page) {
  // axe reads the colours as they are drawn: a menu or a card that is fading in is not at them yet
  await animationsDone(page);
  const results = await new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa"]).analyze();
  return results.violations.filter((v) => v.impact === "serious" || v.impact === "critical");
}

/** A finished thread of `agent`, the box ready for a follow-up. */
async function finished(page: Page, agent?: string) {
  await startThread(page, "echo hi", agent);
  await expect(badge(page)).toHaveText("Done");
}

test("keyboard only: @ opens the agents, the arrows and Enter pick one, Enter sends, and the bubble draws the mention", async ({
  page,
}) => {
  const bodies = posted(page);
  await page.goto("/");
  await box(page).focus();
  await page.keyboard.type("echo ask @");
  // the Coder is the addressed agent: the others are offered
  await expect(list(page)).toBeVisible();
  await expect(list(page).getByRole("option")).toHaveText([/Reviewer/, /Verifier/]);
  await expect(box(page)).toHaveRole("combobox");
  await expect(box(page)).toHaveAttribute("aria-expanded", "true");
  const active = await box(page).getAttribute("aria-activedescendant");
  await expect(page.locator(`[id="${active}"]`)).toHaveText(/Reviewer/);
  await page.keyboard.press("ArrowDown");
  await expect(list(page).getByRole("option", { name: /Verifier/ })).toHaveAttribute(
    "aria-selected",
    "true",
  );
  await page.keyboard.press("Enter");
  await expect(list(page)).toBeHidden();
  await expect(box(page)).toHaveValue("echo ask @verifier ");
  await expect(chips(page)).toHaveText(["Verifier"]);
  // the box is a plain textbox again, and the focus never left it
  await expect(box(page)).toHaveRole("textbox");
  await expect(box(page)).toBeFocused();
  await page.keyboard.type("to look");
  await page.keyboard.press("Enter"); // the list is closed: this sends
  await expect(page).toHaveURL(THREAD_URL);

  expect(bodies).toHaveLength(1);
  const sent = bodies[0]?.messages[0]?.content ?? "";
  expect(sent).toBe("echo ask @verifier to look");
  expect(bodies[0]?.forwardedProps["vymalo.mentions"]).toEqual([
    {
      agentId: "verifier",
      label: "@verifier",
      start: 9,
      end: 18,
      cardUrl: "http://verifier.agents.svc/.well-known/agent-card.json",
    },
  ]);
  await expect(bubbleChips(page)).toHaveText(["@verifier"]);
  await expect(bubbleChips(page)).toHaveAttribute("title", "Verifier (verifier)");
  await expect(badge(page)).toHaveText("Done");
  // and a page opened later reads the same chip from the log
  await page.reload();
  await expect(bubbleChips(page)).toHaveText(["@verifier"]);
  await expectNoHorizontalScroll(page);
});

test("Escape closes the list, Tab picks, and an edit of the label takes the mention away", async ({
  page,
}) => {
  const bodies = posted(page);
  await finished(page, "Reviewer");
  await box(page).focus();
  await page.keyboard.type("echo @co");
  await expect(list(page).getByRole("option")).toHaveText([/Coder/]);
  await page.keyboard.press("Escape");
  await expect(list(page)).toBeHidden();
  await expect(box(page)).toHaveValue("echo @co");
  // the word changes, so the list opens again
  await page.keyboard.type("d");
  await expect(list(page)).toBeVisible();
  await page.keyboard.press("Tab");
  await expect(box(page)).toHaveValue("echo @coder ");
  await expect(chips(page)).toHaveText(["Coder"]);
  // one letter of the label is deleted: it is not a mention any more, and no chip says it is
  await page.keyboard.press("Backspace"); // the space
  await page.keyboard.press("Backspace"); // the r
  await expect(chips(page)).toHaveCount(0);
  await page.keyboard.type("r later");
  await expect(chips(page)).toHaveCount(0);
  await page.keyboard.press("Enter");
  await expect(conversation(page).getByText("echo: echo @coder later")).toBeVisible();
  expect(bodies.at(-1)?.forwardedProps).not.toHaveProperty("vymalo.mentions");
});

test("the warning follows the addressed agent's card: it will not be told, it cannot ask, or nothing", async ({
  page,
}) => {
  // the Reviewer lists neither mentions/v1 nor thread-tools/v1
  await finished(page, "Reviewer");
  await box(page).focus();
  await page.keyboard.type("echo @cod");
  await page.keyboard.press("Enter");
  await expect(warning(page)).toHaveText(
    "Reviewer does not use mentions, so it will not be told who you mentioned. The names stay in your message as text.",
  );
  await page.getByRole("button", { name: "Remove the mention of Coder" }).click();
  await expect(warning(page)).toHaveCount(0);
  await expect(box(page)).toHaveValue("echo ");
  await expect(box(page)).toBeFocused();

  // the Verifier lists mentions/v1 and not thread-tools/v1
  await finished(page, "Verifier");
  await box(page).focus();
  await page.keyboard.type("echo @cod");
  await page.keyboard.press("Enter");
  await expect(warning(page)).toHaveText(
    "Verifier will be told who you mentioned, but it cannot ask other agents.",
  );

  // the Coder lists both: nothing is said
  await finished(page, "Coder");
  await box(page).focus();
  await page.keyboard.type("echo @rev");
  await page.keyboard.press("Enter");
  await expect(chips(page)).toHaveText(["Reviewer"]);
  await expect(warning(page)).toHaveCount(0);
});

test("a card that cannot be read is 'could not check', never 'can'", async ({ page }) => {
  await page.route("**/agui/agents/reviewer/capabilities", (route) =>
    route.fulfill({ status: 500, contentType: "application/problem+json", body: "{}" }),
  );
  await finished(page, "Reviewer");
  await box(page).focus();
  await page.keyboard.type("echo @cod");
  await page.keyboard.press("Enter");
  await expect(warning(page)).toHaveText(
    "Could not check whether Reviewer can work with the agents you mentioned.",
  );
});

test("a message sent while the agent works keeps its mentions", async ({ page }) => {
  const bodies = posted(page);
  await startThread(page, "gate hold", "Coder");
  await expect(badge(page)).toHaveText("Working…");
  await box(page).focus();
  await page.keyboard.type("echo and @rev");
  await page.keyboard.press("Enter"); // a pick, not a send
  await expect(box(page)).toHaveValue("echo and @reviewer ");
  await page.keyboard.press("Enter"); // Send
  await expect(conversation(page).locator('[data-slot="delivery-note"]')).toHaveCount(1);
  expect(bodies).toHaveLength(2);
  expect(bodies[1]?.forwardedProps["vymalo.send"]).toBe("steer");
  expect(bodies[1]?.forwardedProps["vymalo.mentions"]).toMatchObject([
    { agentId: "reviewer", label: "@reviewer", start: 9, end: 18 },
  ]);
  await expect(bubbleChips(page)).toHaveText(["@reviewer"]);
  const id = /\/threads\/([0-9a-f-]{36})$/.exec(page.url())?.[1] ?? "";
  const release = await fetch(`${MOCK_URL}/__mock/release?thread=${id}`, { method: "POST" });
  expect(release.status).toBe(204);
  await expect(badge(page)).toHaveText("Done");
});

test("a refused send says what the orchestrator said and keeps the text with its mention (503, the registry cannot answer)", async ({
  page,
  session,
}) => {
  await session.add({ id: "helper", name: "Helper" });
  await page.goto("/");
  await box(page).focus();
  await page.keyboard.type("echo ask @hel");
  await page.keyboard.press("Enter");
  await expect(chips(page)).toHaveText(["Helper"]);
  await session.down(true);
  await page.keyboard.type("to help");
  await page.keyboard.press("Enter");
  const alert = page.getByRole("alert").filter({ hasText: "the agent registry could not answer" });
  await expect(alert).toBeVisible();
  await expect(box(page)).toHaveValue("echo ask @helper to help");
  // the list was read again, and the registry's agent is not in it now: the chip names it by its id
  await expect(chips(page)).toHaveText(["helper"]);
  await expect(page).not.toHaveURL(THREAD_URL);
  // the registry is back: the same message, sent again, goes out with its mention
  await session.down(false);
  await page.keyboard.press("Enter");
  await expect(page).toHaveURL(THREAD_URL);
  await expect(bubbleChips(page)).toHaveText(["@helper"]);
});

test("a refused send says what the orchestrator said and keeps the text with its mention (422, the roles changed)", async ({
  page,
  session,
}) => {
  await page.goto("/");
  await chooseAgent(page, "Reviewer");
  await box(page).focus();
  await page.keyboard.type("echo ask @cod");
  await page.keyboard.press("Enter");
  await expect(chips(page)).toHaveText(["Coder"]);
  await page.keyboard.type("to look");
  // the person's roles are narrowed after the list was read: of the agents they may invoke, only the Reviewer
  await session.become("limited");
  await page.keyboard.press("Enter");
  const alert = page.getByRole("alert").filter({ hasText: "you may not use 'coder'" });
  await expect(alert).toBeVisible();
  await expect(page).not.toHaveURL(THREAD_URL);
  await expect(box(page)).toHaveValue("echo ask @coder to look");
  await expect(chips(page)).toHaveText(["Coder"]);
});

for (const scheme of ["light", "dark"] as const) {
  test.describe(`accessibility (${scheme})`, () => {
    test.use({ colorScheme: scheme, contextOptions: { reducedMotion: "reduce" } });

    test("axe: the list open, a mention with its chip and each warning have no serious violations", async ({
      page,
    }) => {
      await finished(page, "Reviewer");
      await box(page).focus();
      await page.keyboard.type("echo ask @");
      await expect(list(page)).toBeVisible();
      expect(await axeViolations(page)).toEqual([]);
      await page.keyboard.press("ArrowDown");
      expect(await axeViolations(page)).toEqual([]);
      await page.keyboard.press("Enter");
      await expect(warning(page)).toBeVisible(); // the Reviewer does not use mentions
      await expect(chips(page)).toHaveCount(1);
      expect(await axeViolations(page)).toEqual([]);
      await page.keyboard.press("Enter");
      await expect(bubbleChips(page)).toHaveCount(1);
      expect(await axeViolations(page)).toEqual([]);

      await finished(page, "Verifier");
      await box(page).focus();
      await page.keyboard.type("echo @cod");
      await page.keyboard.press("Enter");
      await expect(warning(page)).toContainText("cannot ask other agents");
      expect(await axeViolations(page)).toEqual([]);
    });
  });
}
