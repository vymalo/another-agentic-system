import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import AxeBuilder from "@axe-core/playwright";
import { chromium, expect, test } from "@playwright/test";
import lighthouse from "lighthouse";
import { uuidv7 } from "../src/lib/uuid";
import { BASE_URL, badge, startThread } from "./helpers";

async function finishedThreadUrl(): Promise<string> {
  // A thread the way any AG-UI client makes one: the consumer mints the id, the POST runs it.
  const id = uuidv7();
  const res = await fetch(`${BASE_URL}/agui/agents/coder`, {
    method: "POST",
    headers: { "Content-Type": "application/json", Accept: "text/event-stream" },
    body: JSON.stringify({
      threadId: id,
      runId: "run-1",
      messages: [{ id: "m-1", role: "user", content: "Implement the thing" }],
    }),
  });
  if (!res.ok) throw new Error(`run refused: ${res.status}`);
  await res.text(); // the response ends with the run
  for (let i = 0; i < 100; i++) {
    const t = (await (await fetch(`${BASE_URL}/api/threads/${id}`)).json()) as { state: string };
    if (t.state === "done") return `${BASE_URL}/threads/${id}`;
    await new Promise((r) => setTimeout(r, 200));
  }
  throw new Error("thread never finished");
}

async function axeViolations(page: import("@playwright/test").Page) {
  const results = await new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa"]).analyze();
  return results.violations.filter((v) => v.impact === "serious" || v.impact === "critical");
}

for (const scheme of ["light", "dark"] as const) {
  test.describe(`accessibility (${scheme})`, () => {
    test.use({ colorScheme: scheme });

    test("axe: new thread page has no serious violations", async ({ page }) => {
      await page.goto("/");
      await expect(page.getByLabel("Agent")).toBeVisible();
      expect(await axeViolations(page)).toEqual([]);
    });

    test("axe: a finished thread has no serious violations", async ({ page }) => {
      await startThread(page, "Implement the thing");
      await expect(badge(page)).toHaveText("Done");
      expect(await axeViolations(page)).toEqual([]);
    });

    test("axe: a blocked thread has no serious violations", async ({ page }) => {
      await startThread(page, "ask which branch");
      await expect(badge(page)).toHaveText("Waiting for you");
      expect(await axeViolations(page)).toEqual([]);
    });

    test("axe: a thread being verified (pending check, counter) has no serious violations", async ({
      page,
    }) => {
      await startThread(page, "verify-wait ship it", "Reviewer");
      await expect(badge(page)).toHaveText("Verifying");
      await expect(
        page.getByRole("region", { name: "Check: CI, attempt 1, pending" }),
      ).toBeVisible();
      expect(await axeViolations(page)).toEqual([]);
    });

    test("axe: a thread waiting for its verifier agent has no serious violations", async ({
      page,
    }) => {
      await startThread(page, "verify-reviewed-wait ship it", "Reviewer");
      await expect(badge(page)).toHaveText("Verifying");
      await expect(
        page.getByRole("region", { name: "Check: Verifier, attempt 1, pending" }),
      ).toBeVisible();
      expect(await axeViolations(page)).toEqual([]);
    });

    test("axe: a finished thread with a replaced and a stale check has no serious violations", async ({
      page,
    }) => {
      await startThread(page, "verify-ci ship it", "Reviewer");
      await expect(badge(page)).toHaveText("Done");
      await expect(
        page.getByRole("region", { name: "Check: CI, attempt 1, failed, stale" }),
      ).toBeVisible();
      expect(await axeViolations(page)).toEqual([]);
    });

    test("axe: a thread that failed its checks (findings, stale-free) has no serious violations", async ({
      page,
    }) => {
      test.setTimeout(60_000);
      await startThread(page, "verify-red fix the login", "Reviewer");
      await expect(badge(page)).toHaveText("Failed", { timeout: 30_000 });
      await expect(page.getByText("Checks failed after 3 attempts")).toBeVisible();
      expect(await axeViolations(page)).toEqual([]);
    });

    test("axe: a refused A2UI surface has no serious violations", async ({ page }) => {
      await startThread(page, "ui-bad now", "Reviewer");
      await expect(badge(page)).toHaveText("Done");
      await page.getByText("Raw operations").click();
      await expect(page.getByText(/Interface not shown:/)).toBeVisible();
      expect(await axeViolations(page)).toEqual([]);
    });

    test("axe: a thread with an A2UI surface has no serious violations (waiting, then finished)", async ({
      page,
    }) => {
      await startThread(page, "ui pick one", "Reviewer");
      await expect(badge(page)).toHaveText("Waiting for you");
      const ui = page.getByRole("region", { name: "Interface from reviewer" });
      await expect(ui.getByRole("button", { name: "Go" })).toBeEnabled();
      expect(await axeViolations(page)).toEqual([]);
      await ui.getByRole("button", { name: "Go" }).click();
      await expect(badge(page)).toHaveText("Done");
      await expect(ui.getByRole("button", { name: "Go" })).toBeDisabled();
      expect(await axeViolations(page)).toEqual([]);
    });
  });
}

test.describe("Lighthouse accessibility on the thread page", () => {
  test.describe.configure({ mode: "serial" });

  const runs = [
    { scheme: "light", port: 9222 },
    { scheme: "dark", port: 9223 },
  ] as const;

  for (const { scheme, port } of runs) {
    test(`score is at least 95 (${scheme})`, async () => {
      test.setTimeout(120_000);
      const url = await finishedThreadUrl();
      // A persistent context is the browser's default context, which is where Lighthouse opens
      // its tab, so the colour scheme emulated here applies to the audited page.
      const userDataDir = mkdtempSync(path.join(tmpdir(), "lh-"));
      const context = await chromium.launchPersistentContext(userDataDir, {
        args: [`--remote-debugging-port=${port}`],
        colorScheme: scheme,
      });
      try {
        const audited: boolean[] = [];
        context.on("page", (p) => {
          p.on("load", () => {
            if (p.url() === url) {
              void p
                .evaluate(() => matchMedia("(prefers-color-scheme: dark)").matches)
                .then((dark) => audited.push(dark))
                .catch(() => {});
            }
          });
        });
        const result = await lighthouse(
          url,
          { port, onlyCategories: ["accessibility"], output: "json", logLevel: "error" },
          {
            extends: "lighthouse:default",
            settings: { formFactor: "desktop", screenEmulation: { disabled: true } },
          },
        );
        expect(
          audited.length,
          "Lighthouse loaded the page in the emulated context",
        ).toBeGreaterThan(0);
        expect(audited.every((dark) => dark === (scheme === "dark"))).toBe(true);
        const lhr = result?.lhr;
        const score = lhr?.categories.accessibility?.score ?? 0;
        const failing = Object.values(lhr?.audits ?? {})
          .filter((a) => a.score !== null && a.score < 1 && a.scoreDisplayMode === "binary")
          .map((a) => `${a.id}: ${a.title}`);
        test.info().annotations.push({ type: "score", description: `${scheme}: ${score}` });
        expect(score, `failing audits: ${failing.join("; ")}`).toBeGreaterThanOrEqual(0.95);
      } finally {
        await context.close();
        rmSync(userDataDir, { recursive: true, force: true });
      }
    });
  }
});
