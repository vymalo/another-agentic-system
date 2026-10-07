import AxeBuilder from "@axe-core/playwright";
import { expect, test } from "@playwright/test";
import { animationsDone, badge, expectNoHorizontalScroll, startThread } from "./helpers";

test("an agent failure is a callout with the agent's reason", async ({ page }) => {
  await startThread(page, "fail please");

  const log = page.getByRole("log", { name: "Conversation" });
  const callout = log.locator('[data-slot="error-callout"]');
  await expect(callout).toContainText("adam couldn’t finish", { ignoreCase: true });
  await expect(callout.getByText("scripted failure", { exact: true })).toBeVisible();
  await expect(badge(page)).toHaveText("Failed");
  // The orchestrator reports an agent failure as `agent_status: failed`, not as an error event.
  await expect(log.getByText("Something went wrong")).toHaveCount(0);
  // not locked: the box stays open, and nothing tells the person to start over
  await expect(page.getByLabel("Message")).toBeEnabled();
  await expect(page.getByText("This thread is failed.")).toHaveCount(0);
  await expect(page.getByRole("link", { name: "Start a new thread" })).toHaveCount(0);
});

test("an undeliverable message shows the error line and waits for the user", async ({ page }) => {
  await startThread(page, "unreachable agent");

  const log = page.getByRole("log", { name: "Conversation" });
  await expect(log.getByText("Something went wrong, and it may pass")).toBeVisible();
  await expect(log.getByText("the agent could not be reached", { exact: false })).toBeVisible();
  await expect(log.getByText("You can send a message to retry.")).toBeVisible();
  // the agent asked nothing: the thread needs a look, not an answer
  await expect(badge(page)).toHaveText("Needs attention");
});

// The owner's exports of 2026-10-06: a failed `yarn check` left a finding of dozens of lines (code frames, a
// stack, a path too long to wrap), shown as one wrapped paragraph that filled the screen on a phone.
for (const scheme of ["light", "dark"] as const) {
  test.describe(`a long failure (${scheme})`, () => {
    test.use({ colorScheme: scheme, contextOptions: { reducedMotion: "reduce" } });

    test("shows its first line, keeps the rest behind Show details, and never widens the page", async ({
      page,
    }) => {
      await startThread(page, "fail-long please");
      await expect(badge(page)).toHaveText("Failed");
      const callout = page
        .getByRole("log", { name: "Conversation" })
        .locator('[data-slot="error-callout"]');
      await expect(callout).toContainText("couldn’t finish", { ignoreCase: true });
      // the message is the first line, alone
      await expect(callout.locator('[data-slot="failure-message"]')).toHaveText(
        "yarn check failed: 3 type errors in 2 files",
      );
      // the rest is behind a disclosure that is closed: none of it is on the screen, and the callout is short
      const details = callout.locator('[data-slot="failure-details"]');
      await expect(details).not.toHaveAttribute("open", "");
      await expect(callout.getByText("Found 3 errors in 2 files.")).toBeHidden();
      await expect(callout.getByText("TS2322", { exact: false })).toBeHidden();
      const height = (await callout.boundingBox())?.height ?? Number.POSITIVE_INFINITY;
      expect(height).toBeLessThan(160);
      await expectNoHorizontalScroll(page);

      // the control is a button the keyboard reaches and works
      const summary = details.locator("summary");
      await expect(summary).toHaveText("Show details", { useInnerText: true });
      await summary.focus();
      await page.keyboard.press("Enter");
      await expect(details).toHaveAttribute("open", "");
      await expect(summary).toHaveText("Hide details", { useInnerText: true });
      const pre = details.locator("pre");
      const scroller = details.locator('[data-slot="failure-scroll"]');
      await expect(pre).toBeVisible();
      await expect(pre).toContainText("src/app/page.tsx:12:7 - error TS2322");
      await expect(pre).toContainText("Found 3 errors in 2 files.");
      // preformatted, monospace, wrapped; the block around it scrolls both ways by itself, and the keyboard can scroll it
      const style = await pre.evaluate((el) => {
        const css = getComputedStyle(el);
        return { whiteSpace: css.whiteSpace, fontFamily: css.fontFamily };
      });
      expect(style.whiteSpace).toBe("pre-wrap");
      expect(style.fontFamily).toMatch(/mono/i);
      const frame = await scroller.evaluate((el) => {
        const css = getComputedStyle(el);
        return {
          maxHeight: css.maxHeight,
          overflowX: css.overflowX,
          overflowY: css.overflowY,
          scrollsDown: el.scrollHeight > el.clientHeight,
          scrollsAcross: el.scrollWidth > el.clientWidth,
        };
      });
      expect(frame.maxHeight).toBe("256px");
      expect([frame.overflowX, frame.overflowY]).toEqual(["auto", "auto"]);
      expect(frame.scrollsDown).toBe(true);
      expect(frame.scrollsAcross).toBe(true);
      await expect(scroller).toHaveAttribute("tabindex", "0");
      await expect(scroller).toHaveAccessibleName("Failure details");
      // all of it stays inside the callout: the page has no horizontal scroll with it open
      await expectNoHorizontalScroll(page);
      const box = await scroller.boundingBox();
      const calloutBox = await callout.boundingBox();
      expect((box?.x ?? 0) + (box?.width ?? 0)).toBeLessThanOrEqual(
        (calloutBox?.x ?? 0) + (calloutBox?.width ?? 0) + 1,
      );
      // and it closes again
      await summary.focus();
      await page.keyboard.press("Space");
      await expect(details).not.toHaveAttribute("open", "");
      await expect(pre).toBeHidden();
    });

    test("axe: the failure, closed and open, has no serious violations", async ({ page }) => {
      await startThread(page, "fail-long please");
      await expect(badge(page)).toHaveText("Failed");
      const violations = async () => {
        await animationsDone(page);
        const results = await new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa"]).analyze();
        return results.violations.filter((v) => v.impact === "serious" || v.impact === "critical");
      };
      expect(await violations()).toEqual([]);
      await page.getByText("Show details").click();
      await expect(page.locator('[data-slot="failure-pre"]')).toBeVisible();
      expect(await violations()).toEqual([]);
    });
  });
}

test("a failure of one short line has no details to show", async ({ page }) => {
  await startThread(page, "fail please");
  const callout = page
    .getByRole("log", { name: "Conversation" })
    .locator('[data-slot="error-callout"]');
  await expect(callout.getByText("scripted failure", { exact: true })).toBeVisible();
  await expect(callout.locator('[data-slot="failure-details"]')).toHaveCount(0);
});
