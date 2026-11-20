import { readFile } from "node:fs/promises";
import path from "node:path";
import { expect, test } from "@playwright/test";
import { GOLDEN_AT, GOLDEN_ID } from "../mock/thread-markdown-golden";
import {
  badge,
  expectNoHorizontalScroll,
  exportMarkdownItem,
  exportMenuItem,
  startThread,
  THREAD_URL,
} from "./helpers";

test("Export JSON downloads the thread as thread-<id>.json", async ({ page }) => {
  await startThread(page, "echo hello");
  await expect(badge(page)).toHaveText("Done");
  const id = /\/threads\/([0-9a-f-]{36})$/.exec(page.url())?.[1];
  expect(page.url()).toMatch(THREAD_URL);

  // Export JSON is an item of the thread's overflow menu
  const button = await exportMenuItem(page);
  await expect(button).toBeEnabled();
  await expectNoHorizontalScroll(page);
  const [download] = await Promise.all([page.waitForEvent("download"), button.click()]);

  expect(download.suggestedFilename()).toBe(`thread-${id}.json`);
  const path = await download.path();
  const doc = JSON.parse(await readFile(path, "utf8"));
  expect(doc.format).toBe("another-agentic-system/thread-export");
  expect(doc.version).toBe(1);
  expect(doc.thread.id).toBe(id);
  expect(doc.thread.state).toBe("done");
  expect(doc.events.length).toBeGreaterThan(0);
  // the web's catalog is the first event of the thread it creates (ADR 0023), then the message
  expect(doc.events[0].kind).toBe("ui_catalog");
  expect(doc.events[1].kind).toBe("user_message");
  expect(doc.events[1].data.text).toBe("echo hello");
  // which builds made it (ADR 0053): every member is there; this web was built with no revision
  expect(doc.version).toBe(1);
  expect(doc.versions.orchestrator.version).toEqual(expect.any(String));
  expect(doc.versions.orchestrator.revision).toEqual(expect.any(String));
  expect(doc.versions.web).toEqual({ revision: "unknown" });
  expect(doc.versions.agents.length).toBeGreaterThan(0);
  await expect(await exportMenuItem(page)).toBeEnabled();
});

test("a failed export says so and keeps the page usable", async ({ page }) => {
  await startThread(page, "echo hello");
  await expect(badge(page)).toHaveText("Done");
  await page.route("**/api/threads/*/export", (route) =>
    route.fulfill({
      status: 503,
      contentType: "application/problem+json",
      body: JSON.stringify({
        title: "Service Unavailable",
        status: 503,
        detail: "storage is unavailable",
      }),
    }),
  );
  await (await exportMenuItem(page)).click();
  await expect(
    page.getByRole("alert").filter({ hasText: "Could not export the thread" }),
  ).toContainText("storage is unavailable");
  await expect(await exportMenuItem(page)).toBeEnabled();
});

/** The golden files of docs/api/thread-markdown-v1.md: the id and the time of the export are the only things a run changes. */
const golden = (name: string) =>
  readFile(
    path.resolve(import.meta.dirname, `../../docs/api/examples/thread-markdown/${name}.md`),
    "utf8",
  );

async function downloadMarkdown(page: import("@playwright/test").Page) {
  const item = await exportMarkdownItem(page);
  await expect(item).toBeEnabled();
  const [download] = await Promise.all([page.waitForEvent("download"), item.click()]);
  return {
    name: download.suggestedFilename(),
    text: await readFile(await download.path(), "utf8"),
  };
}

const fixed = (text: string, id: string) =>
  text
    .replace(id, GOLDEN_ID)
    .replace(/^exported: .*$/m, `exported: ${GOLDEN_AT.replace(/\.\d{3}Z$/, "Z")}`);

for (const [name, message] of [
  ["code-math", "code-math please"],
  ["kept-file", "file make a chart"],
] as const) {
  test(`Export Markdown downloads the file the contract describes (${name})`, async ({ page }) => {
    await startThread(page, message, "Reviewer");
    await expect(badge(page)).toHaveText("Done", { timeout: 20_000 });
    const id = /\/threads\/([0-9a-f-]{36})$/.exec(page.url())?.[1] ?? "";
    const file = await downloadMarkdown(page);
    expect(file.name).toBe(`thread-${id}.md`);
    expect(file.text).toContain(`thread: "${id}"`);
    expect(fixed(file.text, id)).toBe(await golden(name));
    await expect(await exportMarkdownItem(page)).toBeEnabled();
  });
}

test("nothing in the Markdown that the JSON export does not hold", async ({ page }) => {
  await startThread(page, "steps go", "Reviewer");
  await expect(badge(page)).toHaveText("Done", { timeout: 20_000 });
  const id = /\/threads\/([0-9a-f-]{36})$/.exec(page.url())?.[1] ?? "";
  const markdown = (await downloadMarkdown(page)).text;
  // the menu that made the first file is closing: wait for it to be gone, then open it again
  await expect(page.getByRole("menu")).toBeHidden();
  await expect(await exportMenuItem(page)).toBeEnabled();
  const [download] = await Promise.all([
    page.waitForEvent("download"),
    (await exportMenuItem(page)).click(),
  ]);
  const json = JSON.parse(await readFile(await download.path(), "utf8"));
  // the same thread, the same words
  expect(markdown).toContain(`thread: "${json.thread.id}"`);
  expect(json.thread.id).toBe(id);
  expect(markdown).toContain("## *Person*\n\n> steps go");
  expect(markdown).toContain("> Done.");
  // what the JSON has for the steps is there to be read in the JSON; the Markdown counts them and says nothing of them
  const step = json.events.find(
    (e: { kind: string; data: { input?: unknown } }) => e.kind === "agent_step" && e.data.input,
  );
  expect(step).toBeTruthy();
  expect(markdown).toContain("*2 steps, 1 failed.*");
  for (const leak of ["npm test", "NPM_TOKEN", "FAIL src/sum.test.ts", "OpenCode"]) {
    expect(markdown).not.toContain(leak);
  }
  // no link to anything of the signed-in person, and no address
  expect(markdown).not.toMatch(/\/api\/|dev@example\.com/);
});
