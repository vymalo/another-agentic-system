import { expect, type Page, test } from "@playwright/test";
import { allCalls, badge, conversation, resetDb, startThread, threadId } from "./helpers";

/**
 * The Choices component and the UI catalog (ADR 0023) end to end: the web sends its catalog with
 * the run that creates the thread, the orchestrator records it and tells the agent (which lists
 * `ui-catalog/v1`) inline, the agent draws a `Choices` under it and asks, and the person's answers
 * come back as one action the agent reads. The fake agents list the extension for this suite
 * (`FAKE_AGENT_EXTENSIONS` in `playwright.system.config.ts`).
 */

test.beforeEach(resetDb);

const PROP = "vymalo.uiCatalog";

type Run = { messages: unknown[]; resume?: unknown; forwardedProps: Record<string, unknown> };

/** The run POSTs the page makes. */
function runPosts(page: Page): Run[] {
  const bodies: Run[] = [];
  page.on("request", (r) => {
    if (r.method() === "POST" && r.url().includes("/agui/agents/")) {
      bodies.push(r.postDataJSON() as Run);
    }
  });
  return bodies;
}

const ui = (page: Page) => conversation(page).getByRole("region", { name: "Interface from plain" });
const db = (page: Page) => page.getByRole("radiogroup", { name: "Which database?" });
const login = (page: Page) => page.getByRole("radiogroup", { name: "Which login?" });

test("the catalog goes to the agent once, the Choices is drawn under it, and the answers reach the agent", async ({
  page,
}) => {
  const posts = runPosts(page);
  await startThread(page, "choices now", "Plain");
  await expect(badge(page)).toHaveText("Your turn");

  // the run that created the thread carried the web's catalog, whole
  const sent = posts[0]?.forwardedProps[PROP] as {
    catalogId: string;
    version: number;
    digest: string;
    catalog: { components: Record<string, unknown> };
  };
  expect(sent.digest).toMatch(/^sha256:[0-9a-f]{64}$/);
  expect(Object.keys(sent.catalog.components)).toContain("Choices");

  // the agent drew a Choices of three questions
  await expect(ui(page).getByText("A few quick choices")).toBeVisible();
  await expect(db(page).getByRole("radio")).toHaveCount(3); // Postgres, SQLite, Other
  await expect(login(page).getByRole("radio")).toHaveCount(2);

  // answer: one action, no message, and no catalog (the thread has it: its snapshots say so)
  await db(page).getByText("Postgres", { exact: true }).click();
  await login(page).getByText("No login", { exact: true }).click();
  await page.getByRole("button", { name: "Send answers" }).click();
  await expect(badge(page)).toHaveText("Done");
  expect(posts).toHaveLength(2);
  expect(posts[1]?.messages).toEqual([]);
  expect(posts[1]?.resume).toBeUndefined();
  expect(Object.keys(posts[1]?.forwardedProps ?? {})).not.toContain(PROP);
  expect(posts[1]?.forwardedProps).toMatchObject({
    a2uiAction: {
      userAction: {
        name: "answer",
        surfaceId: "s1",
        sourceComponentId: "pick",
        context: {
          answers: [
            { id: "db", values: ["pg"] },
            { id: "auth", values: ["none"] },
            { id: "deploy", values: [] },
          ],
        },
      },
    },
  });

  // the agent read the answers as the action, on the task it asked on
  await expect(
    conversation(page).getByText("answered: ui-action answer db=pg auth=none deploy="),
  ).toHaveCount(1);
  // (the fake agent keeps the calls of every test, so pick this thread's)
  const executions = (await allCalls(page.request, "plain")).filter(
    (c) =>
      c.kind === "execute" &&
      ["choices now", "ui-action answer db=pg auth=none deploy="].includes(c.text),
  );
  expect(executions.map((c) => c.text)).toEqual([
    "choices now",
    "ui-action answer db=pg auth=none deploy=",
  ]);
  expect(executions[1]?.taskId).toBe(executions[0]?.taskId);

  // the agent was told which catalog counts: the whole of it in the first message, the reference
  // in the second (the same digest the web sent)
  expect(executions[0]?.uiCatalog).toEqual({
    catalogId: sent.catalogId,
    version: sent.version,
    digest: sent.digest,
    inline: true,
  });
  // (the agent's copy has its keys in the A2A server's order: compare the sets of components)
  expect(
    executions[0]?.inlineCatalogs.map((c) => [c.catalogId, Object.keys(c.components).sort()]),
  ).toEqual([[sent.catalogId, Object.keys(sent.catalog.components).sort()]]);
  expect(executions[1]?.uiCatalog).toEqual({
    catalogId: sent.catalogId,
    version: sent.version,
    digest: sent.digest,
    inline: false,
  });
  expect(executions[1]?.inlineCatalogs).toEqual([]);

  // the log holds the catalog once, first: it is in the thread's export, attributed to the person
  const res = await page.request.get(`/api/threads/${threadId(page)}/export`);
  expect(res.status()).toBe(200);
  const { events } = (await res.json()) as {
    events: { kind: string; actor: { type: string }; data: Record<string, unknown> }[];
  };
  expect(events.filter((e) => e.kind === "ui_catalog")).toHaveLength(1);
  expect(events[0]?.kind).toBe("ui_catalog");
  expect(events[0]?.actor.type).toBe("user");
  expect(events[0]?.data).toMatchObject({
    catalogId: sent.catalogId,
    version: sent.version,
    digest: sent.digest,
  });

  // a reload replays the answers and the Choices, which does not answer again
  await page.reload();
  await expect(badge(page)).toHaveText("Done");
  await expect(page.getByRole("button", { name: "Send answers" })).toBeDisabled();
  expect(posts).toHaveLength(2);
});
