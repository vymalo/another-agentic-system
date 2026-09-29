import { expect, test } from "@playwright/test";
import { resetDb, THREAD_URL } from "./helpers";

test.beforeEach(resetDb);

test("newest first, Load older pages, selecting navigates", async ({ page }) => {
  test.setTimeout(120_000);
  const total = 51;
  for (let i = 0; i < total; i++) {
    const res = await page.request.post("/api/threads", {
      data: { target: { agentId: "plain" }, text: `echo n${i}` },
    });
    expect(res.status()).toBe(201);
  }
  // let them all finish, so nothing is in flight when the next test truncates the database
  await expect
    .poll(
      async () => {
        const res = await page.request.get("/api/threads?limit=100");
        const threads = (await res.json()) as { state: string }[];
        return threads.filter((t) => t.state === "done").length;
      },
      { timeout: 60_000 },
    )
    .toBe(total);

  await page.goto("/");
  const list = page.getByRole("navigation", { name: "Threads" });
  const rows = list.locator(".threads__item");
  await expect(rows).toHaveCount(50);
  await expect(rows.first()).toHaveText(`echo n${total - 1}`);
  await expect(rows.last()).toHaveText("echo n1");

  await list.getByRole("button", { name: "Load older" }).click();
  await expect(rows).toHaveCount(total);
  await expect(rows.last()).toHaveText("echo n0");
  await expect(list.getByRole("button", { name: "Load older" })).toHaveCount(0);

  await list.getByRole("button", { name: "echo n7", exact: true }).click();
  await expect(page).toHaveURL(THREAD_URL);
  await expect(page.getByRole("heading", { name: "echo n7", exact: true })).toBeVisible();
});
