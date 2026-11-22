import { expect, type Page, test } from "@playwright/test";
import { aPerson, badge, conversation, expectNoHorizontalScroll, MOCK_URL } from "./helpers";

/**
 * A trusted component (ADR 0083, `docs/api/trusted-components-v1.md`) against the mock's `trusted` story: an agent asks
 * the client to draw a key form for the person's own MCP server, the **client** draws its own card — the mark, the
 * sentence that the agent cannot see what is typed, the agent's hints — and what reaches the agent is a **receipt** in
 * the action's `context`, never the key.
 *
 * The page's own calls are read, not the mock's: the `POST /api/user-tool-servers` the card makes with the person's
 * credentials (which the mock keeps), and the run POST that carries the receipt. The secret is in exactly one request
 * body, and in no part of the page afterwards.
 */

const SECRET = "tok-9f3a-this-is-a-secret-value";
const ADDRESS = "https://mcp.example.com/mcp";

type Run = { forwardedProps?: { a2uiAction?: { userAction: Record<string, unknown> } } };
type Sent = { path: string; body: Record<string, unknown> };

const card = (page: Page) =>
  conversation(page).getByRole("region", { name: "Trusted request from adam" });

/** Every run POST the page makes, and every write to the person's own servers. */
function watch(page: Page) {
  const runs: Run[] = [];
  const writes: Sent[] = [];
  page.on("request", (request) => {
    if (request.method() !== "POST") return;
    if (request.url().includes("/agui/agents/")) {
      runs.push(request.postDataJSON() as Run);
      return;
    }
    if (request.url().includes("/api/user-tool-servers")) {
      writes.push({ path: new URL(request.url()).pathname, body: request.postDataJSON() });
    }
  });
  return { runs, writes, actions: () => runs.filter((r) => r.forwardedProps?.a2uiAction) };
}

test("the agent asks for a key, the client draws the form, and only a receipt reaches the agent", async ({
  page,
}, info) => {
  const person = await aPerson(page, info, { me: "tool-maker" });
  const seen = watch(page);

  await page.goto("/");
  await page.getByLabel("Message", { exact: true }).fill("trusted add a key for my search");
  await page.getByRole("button", { name: "Send" }).click();
  await expect(page).toHaveURL(/\/threads\/[0-9a-f-]{36}$/);

  // the client's own card: its sentence, and the agent's hints prefilled and editable
  const region = card(page);
  await expect(region).toBeVisible();
  await expect(region).toContainText("The agent can’t see this.");
  await expect(region.getByLabel("Name")).toHaveValue("My search");
  // the hints are said to be the agent's, and the scopes are chips the person can drop
  await expect(
    region.getByText("Suggested by adam. This is what your Tools menu shows."),
  ).toBeVisible();
  await expect(
    region.getByText(/Suggested by adam\. Your server keeps no scopes of its own/),
  ).toBeVisible();
  await expect(region.getByRole("button", { name: "Remove search.write" })).toBeVisible();
  // the chat waits for the person, and the form waits for what it needs
  await expect(badge(page)).toHaveText("Your turn");
  const save = region.getByRole("button", { name: "Save" });
  await expect(save).toBeDisabled();
  // nothing has been sent before the person acts
  expect(seen.writes).toHaveLength(0);

  // the person drops one suggested scope, types the address and the key
  await region.getByRole("button", { name: "Remove search.write" }).click();
  await expect(region.getByRole("button", { name: "Remove search.write" })).toHaveCount(0);
  await region.getByLabel("Address").fill(ADDRESS);
  await region.getByLabel("Token").fill(SECRET);
  await expect(save).toBeEnabled();
  await save.click();

  // the card called the API itself, with the person's own credentials
  await expect.poll(() => seen.writes.length).toBe(1);
  expect(seen.writes[0]?.path).toBe("/api/user-tool-servers");
  expect(seen.writes[0]?.body).toEqual({
    name: "My search",
    url: ADDRESS,
    auth: { kind: "bearer" },
    secret: SECRET,
  });

  // what the agent got: one action, whose context is the receipt and nothing else
  await expect.poll(() => seen.actions().length).toBe(1);
  const action = seen.actions()[0]?.forwardedProps?.a2uiAction?.userAction;
  expect(action).toMatchObject({
    name: "submit",
    sourceComponentId: "McpCredential",
    context: {
      status: "created",
      id: expect.stringMatching(/^u-[a-z0-9]{10}$/),
      name: "My search",
      scopes: ["search.read"],
    },
  });
  expect(action?.surfaceId).toMatch(/^trusted-\d+$/);
  expect(JSON.stringify(action)).not.toContain(SECRET);

  // the mock kept the server (so the POST reached the route), and the key is nowhere else
  const listed = await fetch(`${MOCK_URL}/api/user-tool-servers`, {
    headers: { cookie: person.cookie },
  });
  expect(listed.status).toBe(200);
  const servers = (await listed.json()) as { servers: { name: string; hasCredential: boolean }[] };
  expect(servers.servers.map((s) => s.name)).toEqual(["My search"]);
  expect(servers.servers[0]?.hasCredential).toBe(true);
  expect(JSON.stringify(servers)).not.toContain(SECRET);

  // the scripted agent echoes the receipt it was given, so the transcript says what arrived
  await expect(conversation(page)).toContainText(/Receipt: \{"status":"created"/);
  await expect(page.locator("body")).not.toContainText(SECRET);
  await expect(page.locator("body")).not.toContainText("tok-9f3a");
  await expectNoHorizontalScroll(page);
});
