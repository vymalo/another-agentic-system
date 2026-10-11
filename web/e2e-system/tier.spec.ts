import { expect, test } from "@playwright/test";
import { badge, callsFor, conversation, framesOf, resetDb, startThread, threadId } from "./helpers";

test.beforeEach(resetDb);

/*
 * The access tier through the real orchestrator (ADR 0061): the coder's card lists `access-tier/v1` with all four tiers (the plain
 * agent's lists none), the orchestrator puts them in the capabilities document the chip is drawn from, checks the pick when the
 * message is written, logs it, and sends it to the agent in the message metadata with the extension activated. The fake agent's
 * `tiered` script echoes the tier it was told, which the orchestrator logs on the agent's status.
 */
const chip = (page: import("@playwright/test").Page) =>
  page.getByRole("button", { name: /^Access tier:/ });

test("the chip exists for the coder and not for the plain agent, and a pick reaches the agent and comes back", async ({
  page,
}) => {
  await page.goto("/");
  await expect(page.getByRole("button", { name: /^Agent:/ })).toBeVisible();
  await expect(chip(page)).toHaveAccessibleName("Access tier: Auto (the agent's default)");

  await chip(page).click();
  await page.getByRole("menuitemradio", { name: /^Plan/ }).click();
  await expect(chip(page)).toHaveAccessibleName("Access tier: Plan");

  const tag = crypto.randomUUID();
  await page.getByLabel("Message").fill(`tiered ${tag}`);
  await page.getByRole("button", { name: "Send" }).click();
  await expect(page).toHaveURL(/\/threads\/[0-9a-f-]{36}$/);
  await expect(badge(page)).toHaveText("Done");

  // the agent: told the tier in the message metadata, the extension activated by the header and by the message
  const [call] = await callsFor(page.request, "coder", `tiered ${tag}`);
  expect(call?.accessTier).toEqual({ tier: "plan" });
  expect(call?.activatesAccessTier).toBe(true);
  // and its words say it worked under it
  await expect(conversation(page).getByText("tiered: plan")).toBeVisible();
  // the bubble: a badge, from what the page sent
  await expect(conversation(page).locator('[data-slot="tier-badge"]')).toHaveText(
    "Sent under the access tier Plan",
  );

  // the frames the real projection writes: the tier on the user message's START, nothing else carries it
  const frames = await framesOf(page.request, threadId(page));
  const start = frames.find(
    (f) => f.event.type === "TEXT_MESSAGE_START" && f.event.role === "user",
  );
  expect(start).toBeDefined();
  const metadata = (start?.event.metadata ?? {}) as Record<string, unknown>;
  expect(metadata["vymalo.tier"]).toBe("plan");

  // a reload: the badge from the log, the pick from this browser
  await page.reload();
  await expect(conversation(page).locator('[data-slot="tier-badge"]')).toHaveText(
    "Sent under the access tier Plan",
  );
  await expect(chip(page)).toHaveAccessibleName("Access tier: Plan");

  // the next message names no new tier: it goes under the same pick, and the agent says so again
  await page.getByLabel("Message").fill(`tiered again ${tag}`);
  await page.getByRole("button", { name: "Send" }).click();
  await expect(conversation(page).getByText("tiered: plan")).toHaveCount(2);

  // the plain agent's card lists nothing: no chip
  await page.goto("/");
  await page.getByRole("button", { name: /^Agent:/ }).click();
  await page.getByRole("menuitemradio", { name: /^Plain/ }).click();
  await expect(page.getByLabel("Message")).toBeVisible();
  await expect(chip(page)).toHaveCount(0);
});

test("a message that names no tier is sent no tier and activates nothing", async ({ page }) => {
  const tag = crypto.randomUUID();
  await startThread(page, `tiered ${tag}`, "Coder");
  await expect(badge(page)).toHaveText("Done");
  const [call] = await callsFor(page.request, "coder", `tiered ${tag}`);
  expect(call?.accessTier).toBeNull();
  expect(call?.activatesAccessTier).toBe(false);
  await expect(conversation(page).getByText("tiered: none")).toBeVisible();
  await expect(conversation(page).locator('[data-slot="tier-badge"]')).toHaveCount(0);
});
