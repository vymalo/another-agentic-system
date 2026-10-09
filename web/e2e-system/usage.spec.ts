import { expect, test } from "@playwright/test";
import { badge, framesOf, resetDb, startThread, threadId } from "./helpers";

test.beforeEach(resetDb);

/*
 * Token usage through the real orchestrator (ADR 0056): the fake agent's `usage` script reports three calls (one under the
 * sub-agent step `Researcher`, one written with doubles) and keeps its totals on the task, which the orchestrator reads with
 * GetTask. The page draws the ring from the frames the real projection writes.
 */
test("the ring is drawn from the orchestrator's usage frames, and an agent's run without usage has none", async ({
  page,
}) => {
  await startThread(page, "usage count the tokens", "Plain");
  await expect(badge(page)).toHaveText("Done");
  const ring = page.getByRole("button", { name: /^Token usage:/ });
  await expect(ring).toHaveAccessibleName("Token usage: context 2 % full, 2,400 of 131,072 tokens");
  await ring.click();
  const details = page.getByRole("dialog", { name: "Token usage" });
  const byModel = details.getByRole("table", { name: "This thread, by model" });
  const row = (name: string) =>
    byModel.getByRole("row").filter({ has: page.getByText(name, { exact: true }) });
  // the task's totals, which only the task held
  await expect(row("glm-5.3")).toContainText("3,600 in");
  await expect(row("glm-5.3-mini")).toContainText("600 in");
  const byWho = details.getByRole("table", { name: "By who spent it" });
  await expect(byWho).toContainText("Plain");
  await expect(byWho).toContainText("Researcher");

  const frames = await framesOf(page.request, threadId(page));
  const usage = frames.filter((f) => f.event.type === "CUSTOM" && f.event.name === "vymalo.usage");
  expect(usage).toHaveLength(3);
  const finished = frames.filter((f) => f.event.type === "RUN_FINISHED").at(-1);
  const said = (finished?.event.usage ?? []) as { model: string }[];
  expect(said.map((u) => u.model)).toEqual(["glm-5.3", "glm-5.3-mini"]);

  // a thread whose agent said no usage: no ring
  await startThread(page, "talk to me", "Plain");
  await expect(badge(page)).toHaveText("Done");
  await expect(page.getByRole("button", { name: /^Token usage:/ })).toHaveCount(0);
});
