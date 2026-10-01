import { expect, test } from "@playwright/test";
import { badge, conversation, startThread } from "./helpers";

/**
 * A2UI surfaces (ADR 0013) against the mock orchestrator, which plays the orchestrator's `ui`
 * story: a surface with a title and a button, the question "Pick one", then the owner's action
 * (`forwardedProps.a2uiAction`) and the answer.
 */

const surfaceOf = (page: import("@playwright/test").Page) =>
  conversation(page).getByRole("region", { name: "Interface from reviewer" });

test.describe("A2UI surfaces", () => {
  test("a surface is drawn, and only a click on its button sends the action", async ({ page }) => {
    const posts: { url: string; body: Record<string, unknown> }[] = [];
    page.on("request", (r) => {
      if (r.method() === "POST" && r.url().includes("/agui/agents/")) {
        posts.push({ url: r.url(), body: r.postDataJSON() as Record<string, unknown> });
      }
    });
    await startThread(page, "ui pick one", "Reviewer");
    await expect(badge(page)).toHaveText("Your turn");

    const ui = surfaceOf(page);
    await expect(ui).toBeVisible();
    await expect(ui.getByText("Pick one")).toBeVisible();
    const go = ui.getByRole("button", { name: "Go" });
    await expect(go).toBeEnabled();
    // labelled by the orchestrator's actor, not by anything in the payload
    await expect(ui.locator('[data-slot="actor-label"]')).toHaveText("reviewer");

    // the surface has been on screen for a while and nothing has been sent but the first message
    await page.waitForTimeout(500);
    expect(posts).toHaveLength(1);
    expect(posts[0]?.body.messages).toHaveLength(1);

    await go.click();
    await expect(badge(page)).toHaveText("Done");
    // the artifact text sits in a collapsed disclosure (in the DOM, hidden)
    await expect(conversation(page).getByText("answered: ui-action go")).toHaveCount(1);
    await expect(conversation(page).getByText("Chose")).toBeVisible();
    expect(posts).toHaveLength(2);
    // the action: no message, no resume, the action of the runtime's convention
    const action = posts[1]?.body as {
      messages: unknown[];
      resume?: unknown;
      forwardedProps: { a2uiAction: { userAction: Record<string, unknown> } };
    };
    expect(action.messages).toEqual([]);
    expect(action.resume).toBeUndefined();
    expect(action.forwardedProps.a2uiAction.userAction).toMatchObject({
      name: "go",
      surfaceId: "s1",
      sourceComponentId: "go",
      context: { choice: "a" },
    });
  });

  test("after the thread finishes the actions are off, and stay off after a reload", async ({
    page,
  }) => {
    await startThread(page, "ui pick one", "Reviewer");
    await expect(badge(page)).toHaveText("Your turn");
    await surfaceOf(page).getByRole("button", { name: "Go" }).click();
    await expect(badge(page)).toHaveText("Done");

    const go = surfaceOf(page).getByRole("button", { name: "Go" });
    await expect(go).toBeDisabled();
    await expect(surfaceOf(page).getByText(/This request is finished/)).toBeVisible();

    await page.reload();
    await expect(badge(page)).toHaveText("Done");
    await expect(surfaceOf(page).getByText("Pick one")).toBeVisible();
    await expect(surfaceOf(page).getByRole("button", { name: "Go" })).toBeDisabled();
    // one surface, not two: the replay of both runs holds it once
    await expect(conversation(page).getByRole("region", { name: /^Interface from/ })).toHaveCount(
      1,
    );
  });

  test("a surface the renderer refuses is one error line: the reason, the raw operations, nothing drawn", async ({
    page,
  }) => {
    await startThread(page, "ui-bad now", "Reviewer");
    await expect(badge(page)).toHaveText("Done");
    const log = conversation(page);
    await expect(log.getByText(/Interface not shown:/)).toBeVisible();
    await expect(log.getByText(/component "Icon" is not in the vocabulary/)).toBeVisible();
    await expect(log.getByRole("region", { name: /^Interface from/ })).toHaveCount(0);
    // nothing of it is drawn: not the title, not the button, not a link
    await expect(log.getByText("Not shown", { exact: true })).toHaveCount(0);
    await expect(log.getByRole("button", { name: "Open", exact: true })).toHaveCount(0);
    await expect(log.getByRole("link", { name: "Open", exact: true })).toHaveCount(0);
    // the raw operations are there, as text, behind a disclosure
    await log.getByText("Raw operations").click();
    await expect(log.getByText("javascript:alert(1)")).toBeVisible();
  });

  test("a surface never draws remote content: no image, no script, nothing fetched from outside", async ({
    page,
  }) => {
    const outside: string[] = [];
    page.on("request", (r) => {
      const url = new URL(r.url());
      if (url.hostname !== "127.0.0.1") outside.push(r.url());
    });
    await startThread(page, "ui pick one", "Reviewer");
    await expect(surfaceOf(page)).toBeVisible();
    expect(await surfaceOf(page).locator("img, iframe, script, object, embed, a").count()).toBe(0);
    expect(outside).toEqual([]);
  });
});
