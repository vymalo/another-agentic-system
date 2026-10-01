import { expect, type Page, test } from "@playwright/test";
import { badge, conversation, startThread } from "./helpers";

/**
 * The UI catalog (ADR 0023) against the mock orchestrator: the web tells a thread about its
 * catalog once, and says out loud what it cannot draw because a newer version of the app opened
 * the thread. `catalog-newer` is the mock's thread of a newer app (catalog version 99).
 */

const PROP = "vymalo.uiCatalog";

type Body = { messages: unknown[]; forwardedProps: Record<string, unknown> };

/** The run POSTs the page makes, in order. */
function runPosts(page: Page): Body[] {
  const bodies: Body[] = [];
  page.on("request", (r) => {
    if (r.method() === "POST" && r.url().includes("/agui/agents/")) {
      bodies.push(r.postDataJSON() as Body);
    }
  });
  return bodies;
}

test.describe("the UI catalog", () => {
  test("the run that creates a thread carries the catalog, whole, and a later run does not", async ({
    page,
  }) => {
    const posts = runPosts(page);
    await startThread(page, "echo first", "Reviewer");
    await expect(badge(page)).toHaveText("Done");

    const sent = posts[0]?.forwardedProps[PROP] as {
      catalogId: string;
      version: number;
      digest: string;
      catalog: { catalogId: string; components: Record<string, unknown> };
    };
    expect(sent.catalogId).toBe("https://agents.vymalo.com/a2ui/catalogs/chat");
    expect(sent.version).toBeGreaterThanOrEqual(1);
    expect(sent.digest).toMatch(/^sha256:[0-9a-f]{64}$/);
    expect(sent.catalog.catalogId).toBe(sent.catalogId);
    expect(Object.keys(sent.catalog.components)).toEqual(
      expect.arrayContaining(["Text", "Column"]),
    );

    // the thread has it now (its snapshots say so): the next job needs no catalog
    await page.getByLabel("Message").fill("echo and more");
    await page.getByRole("button", { name: "Send" }).click();
    await expect(conversation(page).getByText("echo and more").first()).toBeVisible();
    await expect(badge(page)).toHaveText("Done");
    expect(posts).toHaveLength(2);
    expect(Object.keys(posts[1]?.forwardedProps ?? {})).not.toContain(PROP);
  });

  test("a thread opened by a newer version of the app: what needs it is a placeholder, and nothing is sent back", async ({
    page,
  }) => {
    const posts = runPosts(page);
    await startThread(page, "catalog-newer now", "Reviewer");
    await expect(badge(page)).toHaveText("Done");
    const log = conversation(page);

    const note = log.getByRole("group", { name: "Interface needs a newer version of the app" });
    await expect(note).toBeVisible();
    await expect(
      note.getByText("This part of the answer needs a newer version of the app."),
    ).toBeVisible();
    await expect(note.getByText(/“Gizmo”/)).toBeVisible();
    await expect(note.getByRole("button", { name: "Reload" })).toBeEnabled();
    // never half drawn: not the title of the surface, not the refusal
    await expect(log.getByText("Before the gizmo")).toHaveCount(0);
    await expect(log.getByText(/Interface not shown:/)).toHaveCount(0);
    await expect(log.getByRole("region", { name: /^Interface from/ })).toHaveCount(0);

    // the placeholder is still there after a reload (the log is replayed), and so is the version
    await page.reload();
    await expect(badge(page)).toHaveText("Done");
    await expect(note).toBeVisible();

    // an older app does not move the thread back: the next run carries no catalog
    await page.getByLabel("Message").fill("echo after");
    await page.getByRole("button", { name: "Send" }).click();
    await expect(log.getByText("echo after").first()).toBeVisible();
    await expect(badge(page)).toHaveText("Done");
    expect(Object.keys(posts.at(-1)?.forwardedProps ?? {})).not.toContain(PROP);
    // the first run did carry it: the thread was new
    expect(Object.keys(posts[0]?.forwardedProps ?? {})).toContain(PROP);
  });

  test("Reload loads the page again", async ({ page }) => {
    await startThread(page, "catalog-newer now", "Reviewer");
    await expect(badge(page)).toHaveText("Done");
    const reload = page.waitForEvent("load");
    await conversation(page).getByRole("button", { name: "Reload" }).click();
    await reload;
    await expect(badge(page)).toHaveText("Done");
    await expect(conversation(page).getByRole("button", { name: "Reload" })).toBeVisible();
  });

  test("a component the catalog does not have, in a thread that is not newer, is the agent's fault: refused with its rule", async ({
    page,
  }) => {
    await startThread(page, "catalog-unknown now", "Reviewer");
    await expect(badge(page)).toHaveText("Done");
    const log = conversation(page);
    await expect(log.getByText(/Interface not shown:/)).toBeVisible();
    await expect(log.getByText(/component "Gizmo" is not in this app's catalog/)).toBeVisible();
    await expect(log.locator('[data-rule="catalog"]')).toHaveCount(1);
    await expect(log.getByText(/needs a newer version of the app/)).toHaveCount(0);
  });
});
