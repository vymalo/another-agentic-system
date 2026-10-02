import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import AxeBuilder from "@axe-core/playwright";
import { chromium, expect, test } from "@playwright/test";
import lighthouse from "lighthouse";
import { uuidv7 } from "../src/lib/uuid";
import {
  agentPicker,
  BASE_URL,
  badge,
  closeAgentMenu,
  conversation,
  openAgentMenu,
  startThread,
} from "./helpers";

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
    // a turn fades in for 160 ms: axe would read colours that are not at rest yet
    test.use({ colorScheme: scheme, contextOptions: { reducedMotion: "reduce" } });

    test("axe: new thread page has no serious violations", async ({ page }) => {
      await page.goto("/");
      await expect(agentPicker(page)).toBeVisible();
      expect(await axeViolations(page)).toEqual([]);
    });

    test("axe: the agent menu (open, on a new chat and on a thread) has no serious violations", async ({
      page,
    }) => {
      await page.goto("/");
      await openAgentMenu(page);
      await expect(page.getByRole("menuitemradio", { name: /^Coder/ })).toBeChecked();
      expect(await axeViolations(page)).toEqual([]);
      await closeAgentMenu(page);

      await startThread(page, "Implement the thing");
      await expect(badge(page)).toHaveText("Done");
      await openAgentMenu(page);
      await expect(page.getByRole("group", { name: "Agents" })).toBeVisible();
      expect(await axeViolations(page)).toEqual([]);
    });

    test("axe: a thread's agent menu while the agent works (the other agents disabled, with the reason) has no serious violations", async ({
      page,
    }) => {
      await startThread(page, "Refactor the module");
      await expect(badge(page)).toHaveText("Working…");
      await openAgentMenu(page);
      await expect(page.getByText(/The agent is working/)).toBeVisible();
      expect(await axeViolations(page)).toEqual([]);
    });

    test("axe: the turn's actions (shown, and the fork one disabled), the question about another agent and a fork with its divider have no serious violations", async ({
      page,
    }) => {
      // a turn that is going on: Fork from here is disabled
      await startThread(page, "Refactor the module");
      await expect(badge(page)).toHaveText("Working…");
      const fork = conversation(page).getByRole("button", { name: "Fork from here" });
      await expect(fork).toBeDisabled();
      await fork.focus();
      expect(await axeViolations(page)).toEqual([]);
      await page.getByRole("button", { name: "Stop" }).click();
      await expect(badge(page)).toHaveText("Stopped");

      // the question before a fork onto another agent
      await expect(fork).toBeEnabled();
      await openAgentMenu(page);
      await page.getByRole("menuitemradio", { name: /^Reviewer/ }).click();
      // the menu has faded out and the question has faded in: axe reads the colours as they are drawn
      await expect(page.getByRole("menu")).toBeHidden();
      await expect(page.getByRole("alertdialog")).toHaveCSS("opacity", "1");
      expect(await axeViolations(page)).toEqual([]);
      await page.getByRole("button", { name: "Cancel" }).click();
      await expect(page.getByRole("alertdialog")).toBeHidden();

      // the fork: the copied conversation, the divider, the turn's actions
      await fork.click();
      await expect(conversation(page).locator('[data-slot="fork-divider"]')).toBeVisible();
      await expect(badge(page)).toHaveText("Done");
      await conversation(page).getByRole("button", { name: "Fork from here" }).first().focus();
      expect(await axeViolations(page)).toEqual([]);
    });

    test("axe: a finished thread (its composer open) has no serious violations", async ({
      page,
    }) => {
      await startThread(page, "Implement the thing");
      await expect(badge(page)).toHaveText("Done");
      // a thread never locks: the box is there to be checked with the rest of the page
      await expect(page.getByLabel("Message")).toBeEnabled();
      expect(await axeViolations(page)).toEqual([]);
    });

    test("axe: a thread on its second job has no serious violations", async ({ page }) => {
      await startThread(page, "echo first");
      await expect(badge(page)).toHaveText("Done");
      await page.getByLabel("Message").fill("echo and now more");
      await page.getByRole("button", { name: "Send" }).click();
      await expect(conversation(page).getByText("echo and now more")).toBeVisible();
      await expect(badge(page)).toHaveText("Done");
      await expect(page.getByLabel("Message")).toBeEnabled();
      expect(await axeViolations(page)).toEqual([]);
    });

    test("axe: a blocked thread has no serious violations", async ({ page }) => {
      await startThread(page, "ask which branch");
      await expect(badge(page)).toHaveText("Your turn");
      expect(await axeViolations(page)).toEqual([]);
    });

    test("axe: a thread being verified (pending check, counter) has no serious violations", async ({
      page,
    }) => {
      await startThread(page, "verify-wait ship it", "Reviewer");
      await expect(badge(page)).toHaveText("Checking the work…");
      await expect(
        page.getByRole("listitem", { name: "Check: CI, attempt 1, pending" }),
      ).toBeVisible();
      expect(await axeViolations(page)).toEqual([]);
    });

    test("axe: a thread waiting for its verifier agent has no serious violations", async ({
      page,
    }) => {
      await startThread(page, "verify-reviewed-wait ship it", "Reviewer");
      await expect(badge(page)).toHaveText("Checking the work…");
      await expect(
        page.getByRole("listitem", { name: "Check: Verifier, attempt 1, pending" }),
      ).toBeVisible();
      expect(await axeViolations(page)).toEqual([]);
    });

    test("axe: a finished thread with a replaced and a stale check has no serious violations", async ({
      page,
    }) => {
      await startThread(page, "verify-ci-stale ship it", "Reviewer");
      // eleven steps 400 ms apart: about the default wait, too close under a loaded runner
      await expect(badge(page)).toHaveText("Done", { timeout: 20_000 });
      await expect(
        page.getByRole("listitem", { name: "Check: CI, attempt 1, failed, stale" }),
      ).toBeVisible();
      expect(await axeViolations(page)).toEqual([]);
    });

    test("axe: a finished thread with CI report cards (red, then green, with links) has no serious violations", async ({
      page,
    }) => {
      test.setTimeout(60_000);
      await startThread(page, "verify-ci fix the login", "Reviewer");
      await expect(badge(page)).toHaveText("Done", { timeout: 30_000 });
      await expect(page.getByRole("listitem", { name: "CI: ci/build, failure" })).toBeVisible();
      await expect(page.getByRole("listitem", { name: "CI: ci/build, success" })).toBeVisible();
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

    test("axe: the placeholder of a surface that needs a newer version of the app has no serious violations", async ({
      page,
    }) => {
      await startThread(page, "catalog-newer now", "Reviewer");
      await expect(badge(page)).toHaveText("Done");
      await expect(
        page.getByRole("group", { name: "Interface needs a newer version of the app" }),
      ).toBeVisible();
      expect(await axeViolations(page)).toEqual([]);
    });

    test("axe: a Choices (unanswered, answered but not sent, and the person's answers) has no serious violations", async ({
      page,
    }) => {
      await startThread(page, "choices now", "Reviewer");
      await expect(badge(page)).toHaveText("Your turn");
      await expect(page.getByRole("radiogroup", { name: "Which database?" })).toBeVisible();
      expect(await axeViolations(page)).toEqual([]);
      await page.getByText("Postgres", { exact: true }).click();
      await page.getByText("Keycloak", { exact: true }).click();
      await page.getByLabel("Your own answer to: Where does it run?").fill("bare metal");
      await expect(page.getByRole("button", { name: "Send answers" })).toBeEnabled();
      expect(await axeViolations(page)).toEqual([]);
      await page.getByRole("button", { name: "Send answers" }).click();
      await expect(badge(page)).toHaveText("Done");
      await expect(page.locator('[data-slot="answer-bubble"]')).toBeVisible();
      expect(await axeViolations(page)).toEqual([]);
    });

    test("axe: an answer of words, cards and a graph has no serious violations (the graph drawn, its source open)", async ({
      page,
    }) => {
      await startThread(page, "cards-mermaid please", "Reviewer");
      await expect(badge(page)).toHaveText("Done");
      const ui = page.getByRole("region", { name: "Interface from reviewer" });
      await expect(ui.getByRole("img", { name: "How a request meets a session" })).toBeVisible();
      await expect(ui.getByRole("list").getByRole("listitem")).toHaveCount(3);
      expect(await axeViolations(page)).toEqual([]);
      await ui.getByText("Diagram source").click();
      await expect(ui.locator('[data-slot="mermaid-code"]')).toBeVisible();
      expect(await axeViolations(page)).toEqual([]);
    });

    test("axe: a graph that does not parse, and a refused Cards, have no serious violations", async ({
      page,
    }) => {
      await startThread(page, "mermaid-bad now", "Reviewer");
      await expect(badge(page)).toHaveText("Done");
      await expect(page.getByText("The graph could not be drawn.")).toBeVisible();
      expect(await axeViolations(page)).toEqual([]);
      await startThread(page, "cards-bad now", "Reviewer");
      await expect(badge(page)).toHaveText("Done");
      await expect(page.getByText(/Interface not shown:/)).toBeVisible();
      expect(await axeViolations(page)).toEqual([]);
    });

    test("axe: a thread with an A2UI surface has no serious violations (waiting, then finished)", async ({
      page,
    }) => {
      await startThread(page, "ui pick one", "Reviewer");
      await expect(badge(page)).toHaveText("Your turn");
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

/**
 * Lighthouse's accessibility audit of `url` in a browser of the given colour scheme and form factor.
 * `watch` is a selector that must have been on the audited page at some point of the audit.
 */
async function lighthouseScore(opts: {
  url: string;
  scheme: "light" | "dark";
  port: number;
  mobile?: boolean;
  watch?: string;
}): Promise<{ score: number; failing: string[]; saw: boolean }> {
  const { url, scheme, port, mobile = false, watch } = opts;
  // A persistent context is the browser's default context, which is where Lighthouse opens
  // its tab, so the colour scheme emulated here applies to the audited page.
  const userDataDir = mkdtempSync(path.join(tmpdir(), "lh-"));
  const context = await chromium.launchPersistentContext(userDataDir, {
    args: [`--remote-debugging-port=${port}`],
    colorScheme: scheme,
  });
  try {
    const audited: boolean[] = [];
    let saw = false;
    const watchers: ReturnType<typeof setInterval>[] = [];
    context.on("page", (p) => {
      p.on("load", () => {
        if (p.url() !== url) return;
        void p
          .evaluate(() => matchMedia("(prefers-color-scheme: dark)").matches)
          .then((dark) => audited.push(dark))
          .catch(() => {});
        if (!watch) return;
        watchers.push(
          setInterval(() => {
            void p
              .evaluate((selector) => document.querySelector(selector) !== null, watch)
              .then((found) => {
                if (found) saw = true;
              })
              .catch(() => {});
          }, 250),
        );
      });
    });
    const result = await lighthouse(
      url,
      { port, onlyCategories: ["accessibility"], output: "json", logLevel: "error" },
      {
        extends: "lighthouse:default",
        settings: mobile
          ? { formFactor: "mobile" }
          : { formFactor: "desktop", screenEmulation: { disabled: true } },
      },
    );
    for (const w of watchers) clearInterval(w);
    expect(audited.length, "Lighthouse loaded the page in the emulated context").toBeGreaterThan(0);
    expect(audited.every((dark) => dark === (scheme === "dark"))).toBe(true);
    const lhr = result?.lhr;
    const failing = Object.values(lhr?.audits ?? {})
      .filter((a) => a.score !== null && a.score < 1 && a.scoreDisplayMode === "binary")
      .map((a) => `${a.id}: ${a.title}`);
    return { score: lhr?.categories.accessibility?.score ?? 0, failing, saw };
  } finally {
    await context.close();
    rmSync(userDataDir, { recursive: true, force: true });
  }
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
      const { score, failing } = await lighthouseScore({ url, scheme, port });
      test.info().annotations.push({ type: "score", description: `${scheme}: ${score}` });
      expect(score, `failing audits: ${failing.join("; ")}`).toBeGreaterThanOrEqual(0.95);
    });
  }
});

/** A thread whose agent is writing its reply and never finishes (until Stop): the draft is on the page. */
async function writingThreadUrl(): Promise<{ url: string; id: string }> {
  const id = uuidv7();
  const res = await fetch(`${BASE_URL}/agui/agents/coder`, {
    method: "POST",
    headers: { "Content-Type": "application/json", Accept: "text/event-stream" },
    body: JSON.stringify({
      threadId: id,
      runId: "run-1",
      messages: [{ id: "m-1", role: "user", content: "stream-hold write the plan" }],
    }),
  });
  if (!res.ok) throw new Error(`run refused: ${res.status}`);
  await res.body?.cancel();
  return { url: `${BASE_URL}/threads/${id}`, id };
}

// The words of a reply that is still being written (live text, ADR 0027) are on the page: a draft
// the page was told by the sender's refresh a moment after it connected, with its caret.
test.describe("Lighthouse accessibility with a reply being written", () => {
  test.describe.configure({ mode: "serial" });

  const runs = [
    { scheme: "light", mobile: false, port: 9224 },
    { scheme: "dark", mobile: false, port: 9225 },
    { scheme: "light", mobile: true, port: 9226 },
    { scheme: "dark", mobile: true, port: 9227 },
  ] as const;

  for (const { scheme, mobile, port } of runs) {
    const device = mobile ? "phone" : "desktop";
    test(`score is at least 95 (${scheme}, ${device})`, async () => {
      test.setTimeout(120_000);
      const { url, id } = await writingThreadUrl();
      try {
        const { score, failing, saw } = await lighthouseScore({
          url,
          scheme,
          port,
          mobile,
          watch: '[data-slot="agent-draft"]',
        });
        expect(saw, "the audited page showed the draft").toBe(true);
        test
          .info()
          .annotations.push({ type: "score", description: `${scheme} ${device}: ${score}` });
        expect(score, `failing audits: ${failing.join("; ")}`).toBeGreaterThanOrEqual(0.95);
      } finally {
        await fetch(`${BASE_URL}/api/threads/${id}/cancel`, { method: "POST" });
      }
    });
  }
});
