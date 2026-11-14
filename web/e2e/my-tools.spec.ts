import AxeBuilder from "@axe-core/playwright";
import { test as base, expect, type Locator, type Page } from "@playwright/test";
import {
  animationsDone,
  errorLine,
  expectNoHorizontalScroll,
  MOCK_URL,
  THREAD_URL,
} from "./helpers";

/*
 * "My tool servers" (ADR 0046, web/DESIGN.md "My tool servers"), against the mock's people's own servers: the page, the
 * form, the row, Connect by OAuth, the picker and the menu item. The mock keeps a person's servers and the deployment's rules
 * for them per session, named by a cookie, so each test has its own and nobody else's. A test reaches the same routes the
 * page does (with the session's cookie) to set up what it needs and to change a server "in another tab".
 */

const ORIGIN = "http://127.0.0.1:3000";
const SECRET = "tok-9f3a-this-is-a-secret-value";
const NEW_SECRET = "tok-2b7c-another-secret-value";

type Rules = Record<string, unknown>;

type Mine = {
  session: string;
  /** Who the session is (`tool-maker`, `user`, `no-access`, ...). */
  as: (profile: string) => Promise<void>;
  /** The deployment's rules for the feature (`POST /__mock/user-tools`). */
  rules: (rules: Rules) => Promise<void>;
  /** A call to the mock's own routes with this session's cookie: what "another tab" does. */
  call: (method: string, path: string, body?: unknown) => Promise<Response>;
  /** A server added behind the page's back. */
  seed: (body: unknown) => Promise<{ id: string; revision: string; name: string }>;
};

const test = base.extend<{ mine: Mine }>({
  mine: async ({ context }, use, info) => {
    const session = `my-tools-${info.workerIndex}-${info.testId}`;
    await context.addCookies([{ name: "mock-registry", value: session, url: ORIGIN }]);
    const hook = async (path: string, body?: unknown) => {
      const res = await fetch(
        `${MOCK_URL}${path}${path.includes("?") ? "&" : "?"}session=${session}`,
        {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          ...(body === undefined ? {} : { body: JSON.stringify(body) }),
        },
      );
      expect(res.ok).toBe(true);
    };
    const call: Mine["call"] = (method, path, body) =>
      fetch(`${MOCK_URL}${path}`, {
        method,
        headers: { "Content-Type": "application/json", Cookie: `mock-registry=${session}` },
        ...(body === undefined ? {} : { body: JSON.stringify(body) }),
      });
    await hook("/__mock/config?me=tool-maker");
    await use({
      session,
      as: (profile) => hook(`/__mock/config?me=${profile}`),
      rules: (rules) => hook("/__mock/user-tools", rules),
      call,
      seed: async (body) => {
        const res = await call("POST", "/api/user-tool-servers", body);
        expect(res.status).toBe(201);
        return (await res.json()) as { id: string; revision: string; name: string };
      },
    });
  },
});

const bearer = (name: string, host = "mcp.example.com") => ({
  name,
  url: `https://${host}/mcp`,
  auth: { kind: "bearer" },
  secret: SECRET,
});
const oauth = (name: string, host = "oauth.example.com") => ({
  name,
  url: `https://${host}/mcp`,
  auth: { kind: "oauth2", scopes: ["read"] },
});

const rowOf = (page: Page, name: string): Locator =>
  page
    .locator('[data-slot="server-row"]')
    .filter({ has: page.getByRole("heading", { name, exact: true }) });

async function fillAdd(
  page: Page,
  fields: { name: string; url: string; kind: string; secret?: string; header?: string },
) {
  await page.getByLabel("Name", { exact: true }).fill(fields.name);
  await page.getByLabel("Address").fill(fields.url);
  await page.getByLabel("Authentication").selectOption(fields.kind);
  if (fields.header) await page.getByLabel("Header name").fill(fields.header);
  if (fields.secret) await page.getByLabel(/^(Token|API key)$/).fill(fields.secret);
}

test("a bearer server is added and its secret is never shown again", async ({ page, mine }) => {
  // what the page sends and is sent: the secret goes in one request body, and in no address and no answer
  const sentIn: string[] = [];
  const addresses: string[] = [];
  const answers: string[] = [];
  page.on("request", (request) => {
    addresses.push(request.url());
    if ((request.postData() ?? "").includes(SECRET))
      sentIn.push(`${request.method()} ${request.url()}`);
  });
  page.on("response", async (response) => {
    if (!response.url().includes("/api/")) return;
    answers.push(await response.text().catch(() => ""));
  });
  await page.goto("/tools");
  await expect(page.getByRole("heading", { level: 1, name: "My tool servers" })).toBeVisible();
  await expect(page.getByText("You have not added a server yet.")).toBeVisible();

  await fillAdd(page, {
    name: "Search",
    url: "https://search.example.com/mcp",
    kind: "bearer",
    secret: SECRET,
  });
  await expect(page.getByLabel("Token")).toHaveAttribute("type", "password");
  await page.getByRole("button", { name: "Add server" }).click();

  const row = rowOf(page, "Search");
  await expect(row).toBeVisible();
  await expect(row.locator('[data-slot="status"]')).toContainText("Ready.");
  await expect(row).toContainText("Bearer token");
  // only the host: the path of an address can hold a secret
  await expect(row).toContainText("search.example.com");
  await expect(row).not.toContainText("/mcp");
  await expect(page.getByText("Added “Search”.")).toBeVisible();

  // the form is empty again, and the page holds the secret nowhere
  await page.getByRole("button", { name: "Add a server" }).click();
  await page.getByLabel("Authentication").selectOption("bearer");
  await expect(page.getByLabel("Token")).toHaveValue("");
  expect(await page.content()).not.toContain(SECRET);
  expect(
    await page.evaluate(() =>
      JSON.stringify([{ ...localStorage }, { ...sessionStorage }, location.href]),
    ),
  ).not.toContain(SECRET);

  // the one request that carried it was the add; no address and no answer ever did
  await page.waitForLoadState("networkidle");
  expect(sentIn).toHaveLength(1);
  expect(sentIn[0]).toMatch(/^POST .*\/api\/user-tool-servers$/);
  expect(addresses.join("\n")).not.toContain(SECRET);
  expect(answers.join("\n")).not.toContain(SECRET);

  // after a reload the server is there and there is still no secret: the page is only what the API says
  await page.reload();
  await expect(rowOf(page, "Search")).toBeVisible();
  expect(await page.content()).not.toContain(SECRET);
  // and editing it does not fill one in
  await rowOf(page, "Search").getByRole("button", { name: "Edit Search" }).click();
  await expect(page.getByLabel("New token")).toHaveValue("");
  await expectNoHorizontalScroll(page);
  // the API itself says only that a credential is kept
  const listed = await (await mine.call("GET", "/api/user-tool-servers")).text();
  expect(listed).toContain('"hasCredential":true');
  expect(listed).not.toContain(SECRET);
});

test("a stale re-key is refused in words, and the list is read again", async ({ page, mine }) => {
  const server = await mine.seed(bearer("Search"));
  await page.goto("/tools");
  const row = rowOf(page, "Search");
  await expect(row).toBeVisible();
  // another tab renames it: the revision the page holds is now stale
  const other = await mine.call("PATCH", `/api/user-tool-servers/${server.id}`, {
    name: "Renamed elsewhere",
    revision: server.revision,
  });
  expect(other.status).toBe(200);

  await row.getByRole("button", { name: "Replace the key of Search" }).click();
  await page.getByLabel("New token").fill(NEW_SECRET);
  await page.getByRole("button", { name: "Save key" }).click();
  await expect(errorLine(page)).toContainText("changed since you opened it");
  // the secret is gone from the field: the person types it again
  await expect(page.getByLabel("New token")).toHaveValue("");
  // the list was read again: the row is what the other tab made it
  await expect(rowOf(page, "Renamed elsewhere")).toBeVisible();
  await page.getByRole("button", { name: "Cancel" }).click();

  // and a re-key from the fresh row is made
  await rowOf(page, "Renamed elsewhere")
    .getByRole("button", { name: "Replace the key of Renamed elsewhere" })
    .click();
  await page.getByLabel("New token").fill(NEW_SECRET);
  await page.getByRole("button", { name: "Save key" }).click();
  await expect(page.getByLabel("New token")).toHaveCount(0);
  await expect(rowOf(page, "Renamed elsewhere").locator('[data-slot="status"]')).toContainText(
    "Ready.",
  );
  expect(await page.content()).not.toContain(NEW_SECRET);
});

test("the ninth is refused, in words, and the form goes away at the limit", async ({
  page,
  mine,
}) => {
  for (let i = 0; i < 7; i++) await mine.seed(bearer(`Server ${i}`, `s${i}.example.com`));
  await page.goto("/tools");
  await expect(page.getByRole("heading", { level: 2, name: /Your servers/ })).toContainText(
    "7 of 8",
  );
  // another tab adds the eighth while this page still has room
  await mine.seed(bearer("Server 7", "s7.example.com"));
  await page.getByRole("button", { name: "Add a server" }).click();
  await fillAdd(page, { name: "Ninth", url: "https://ninth.example.com/mcp", kind: "none" });
  await page.getByRole("button", { name: "Add server" }).click();
  // the server refused the ninth (409 `too_many_servers`), and the list is read again: eight, and no way to add another
  await expect(page.getByRole("heading", { level: 2, name: /Your servers/ })).toContainText(
    "8 of 8",
  );
  await expect(page.getByText(/You have 8 servers, the most one person can have/)).toBeVisible();
  await expect(page.getByRole("button", { name: "Add a server" })).toHaveCount(0);
  await expect(page.getByLabel("Address")).toHaveCount(0);
  await expect(rowOf(page, "Ninth")).toHaveCount(0);
});

test("a refused add leaves the secret nowhere: not in the page, not in any field, not in storage, not in the console", async ({
  page,
  mine,
}) => {
  const console_: string[] = [];
  page.on("console", (m) => console_.push(m.text()));
  page.on("pageerror", (e) => console_.push(String(e)));
  const nowhere = async (why: string) => {
    const where = await page.evaluate(() => ({
      text: document.body.innerText,
      values: [...document.querySelectorAll("input, textarea, select")].map(
        (i) => (i as HTMLInputElement).value,
      ),
      stored: JSON.stringify([{ ...localStorage }, { ...sessionStorage }, location.href]),
    }));
    expect(where.text, why).not.toContain(SECRET);
    expect(where.values.join("|"), why).not.toContain(SECRET);
    expect(where.stored, why).not.toContain(SECRET);
    expect(console_.join("\n"), why).not.toContain(SECRET);
  };
  // 7 servers, so the page has room and another tab can take the last place
  for (let i = 0; i < 7; i++) await mine.seed(bearer(`S${i}`, `s${i}.example.com`));
  await page.goto("/tools");
  await page.getByRole("button", { name: "Add a server" }).click();
  const add = async (secret: string) => {
    await fillAdd(page, {
      name: "New",
      url: "https://new.example.com/mcp",
      kind: "bearer",
      secret,
    });
    await page.getByRole("button", { name: "Add server" }).click();
  };
  // a 400 (the secret is too short for the server) quotes nothing
  await add("short");
  await expect(errorLine(page)).toBeVisible();
  await nowhere("after a 400");
  // a 503 from the credential broker
  await page.route("**/api/user-tool-servers", (route) =>
    route.request().method() === "POST"
      ? route.fulfill({
          status: 503,
          contentType: "application/problem+json",
          body: JSON.stringify({ title: "Unavailable", status: 503, code: "broker_down" }),
        })
      : route.continue(),
  );
  await add(SECRET);
  await expect(errorLine(page)).toContainText("could not be kept or removed");
  await nowhere("after a 503");
  await page.unroute("**/api/user-tool-servers");
  // a 409 (another tab took the last place)
  await mine.seed(bearer("S7", "s7.example.com"));
  await add(SECRET);
  await expect(page.getByText(/You have 8 servers/)).toBeVisible();
  await nowhere("after a 409");
  expect(console_.join("\n")).not.toContain(SECRET);
});

test("an address the deployment refuses is said in words and keeps what was typed", async ({
  page,
  mine,
}) => {
  await page.goto("/tools");
  await fillAdd(page, { name: "Inside", url: "https://10.0.0.5/mcp", kind: "none" });
  await page.getByRole("button", { name: "Add server" }).click();
  await expect(errorLine(page)).toContainText("That address was refused");
  // the words are tied to the Address field, and the focus is there
  await expect(page.getByLabel("Address")).toHaveAttribute("aria-invalid", "true");
  await expect(page.getByLabel("Address")).toBeFocused();
  await expect(page.getByLabel("Name", { exact: true })).toHaveValue("Inside");
  await expect(rowOf(page, "Inside")).toHaveCount(0);
  // nothing was written
  const listed = (await (await mine.call("GET", "/api/user-tool-servers")).json()) as {
    servers: unknown[];
  };
  expect(listed.servers).toEqual([]);
});

test("oauth2 shows the issuer before anything opens, then turns ready when the person comes back", async ({
  page,
  context,
  mine,
}) => {
  await mine.rules({ issuer: "https://login.stranger.example" });
  await page.goto("/tools");
  await fillAdd(page, { name: "Notes", url: "https://notes.example.com/mcp", kind: "oauth2" });
  await page.getByRole("button", { name: "Add server" }).click();
  const row = rowOf(page, "Notes");
  await expect(row.locator('[data-slot="status"]')).toContainText("Needs a sign-in.");

  await row.getByRole("button", { name: "Connect Notes" }).click();
  const dialog = page.getByRole("alertdialog");
  // where they are about to be sent, said first: the issuer's host, and whose choice it was
  await expect(dialog).toContainText("login.stranger.example");
  await expect(dialog).toContainText("chosen by the server you added, not by this deployment");
  expect(context.pages()).toHaveLength(1);
  // the way on is a real link to exactly the host the dialog names; nothing has been opened yet
  const go = dialog.getByRole("link", { name: "Continue to 127.0.0.1:4010" });
  await expect(go).toHaveAttribute("target", "_blank");
  await expect(go).toHaveAttribute("rel", "noopener noreferrer");
  await expect(dialog).toContainText("You will be sent to 127.0.0.1:4010");

  const [consent] = await Promise.all([context.waitForEvent("page"), go.click()]);
  await consent.waitForLoadState("domcontentloaded");
  // the stand-in for the authorization server sent the browser to the callback, which says so in fixed words
  await expect(consent.getByRole("heading", { name: "The server is connected" })).toBeVisible();
  expect(new URL(consent.url()).pathname).toBe("/api/user-tool-servers/oauth/callback");
  await consent.close();

  // coming back (the window regains focus) reads the list
  await page.bringToFront();
  await page.evaluate(() => window.dispatchEvent(new Event("focus")));
  await expect(row.locator('[data-slot="status"]')).toContainText("Ready.");
  await expect(row).not.toContainText("Waiting for you");
  expect(await page.content()).not.toContain("state=");
});

test("a person who declines is told nothing was connected, and the row is as it was", async ({
  page,
  context,
  mine,
}) => {
  await mine.seed(oauth("Notes"));
  await page.goto("/tools");
  const row = rowOf(page, "Notes");
  // the person says no at the authorization server: the stand-in's approval is turned into a refusal on its way
  await context.route("**/__mock/oauth/approve**", (route) =>
    route.continue({ url: `${route.request().url()}&decline=1` }),
  );
  await row.getByRole("button", { name: "Connect Notes" }).click();
  const [consent] = await Promise.all([
    context.waitForEvent("page"),
    page
      .getByRole("alertdialog")
      .getByRole("link", { name: /^Continue to/ })
      .click(),
  ]);
  await expect(
    consent.getByRole("heading", { name: "The authorization was not completed" }),
  ).toBeVisible();
  await consent.close();
  await expect(row).toContainText("Waiting for you to finish signing in");
  await row.getByRole("button", { name: "Check now" }).click();
  await expect(row.locator('[data-slot="status"]')).toContainText("Needs a sign-in.");
});

test("Connect says in words why the sign-in could not be started", async ({ page, mine }) => {
  await mine.seed(oauth("Notes"));
  await page.goto("/tools");
  const row = rowOf(page, "Notes");
  await mine.rules({ authorize: "oauth_not_configured" });
  await row.getByRole("button", { name: "Connect Notes" }).click();
  await expect(row.getByRole("alert")).toContainText("Ask whoever runs it");
  await expect(page.getByRole("alertdialog")).toHaveCount(0);
  await mine.rules({ authorize: "egress_refused" });
  await row.getByRole("button", { name: "Connect Notes" }).click();
  await expect(row.getByRole("alert")).toContainText(
    "does not allow connections to an address the sign-in needs",
  );
});

test("a server that needs a key is given one, and a re-key of one that ended brings it back", async ({
  page,
  mine,
}) => {
  const server = await mine.seed(bearer("Search"));
  await mine.rules({ mark: { name: "Search", state: "needs_reauth" } });
  await page.goto("/tools");
  const row = rowOf(page, "Search");
  await expect(row.locator('[data-slot="status"]')).toContainText("Needs a key.");
  await expect(row.locator('[data-slot="status"]')).toContainText("The key was refused or ended");
  await row.getByRole("button", { name: "Add a key to Search" }).click();
  await page.getByLabel("New token").fill(NEW_SECRET);
  await page.getByRole("button", { name: "Save key" }).click();
  await expect(row.locator('[data-slot="status"]')).toContainText("Ready.");
  // the revision moved: it was a write
  const listed = (await (await mine.call("GET", "/api/user-tool-servers")).json()) as {
    servers: { id: string; revision: string }[];
  };
  expect(listed.servers[0]?.revision).not.toBe(server.revision);
});

test("an address that is moved says it forgets the credential before it is sent", async ({
  page,
  mine,
}) => {
  await mine.seed(bearer("Search"));
  await page.goto("/tools");
  await rowOf(page, "Search").getByRole("button", { name: "Edit Search" }).click();
  await page.getByLabel("Address").fill("https://moved.example.com/mcp");
  await expect(
    page.getByRole("status").filter({ hasText: "forget its current key or sign-in" }),
  ).toBeVisible();
  await page.getByRole("button", { name: "Save changes" }).click();
  await expect(rowOf(page, "Search").locator('[data-slot="status"]')).toContainText("Needs a key.");
});

test("a server is deleted after a question, and the list says it is gone", async ({
  page,
  mine,
}) => {
  await mine.seed(bearer("Search"));
  await page.goto("/tools");
  await rowOf(page, "Search").getByRole("button", { name: "Delete Search" }).click();
  const dialog = page.getByRole("alertdialog");
  await expect(dialog).toContainText("Delete “Search”?");
  await dialog.getByRole("button", { name: "Delete", exact: true }).click();
  await expect(rowOf(page, "Search")).toHaveCount(0);
  await expect(page.getByText("You have not added a server yet.")).toBeVisible();
});

test("attach from the picker: a ready server is checked and marked as theirs, one that needs setup leads to My tool servers", async ({
  page,
  mine,
}) => {
  const ready = await mine.seed(bearer("My search"));
  await mine.seed(oauth("My notes"));
  await page.goto("/");
  await page.getByRole("button", { name: "Tools", exact: true }).click();
  const menu = page.getByRole("menu");
  const search = menu.getByRole("menuitemcheckbox", { name: /^My search/ });
  await expect(search).toContainText("Yours");
  // a deployment's own server is not marked
  await expect(menu.getByRole("menuitemcheckbox", { name: /^Web search/ })).not.toContainText(
    "Yours",
  );
  const notes = menu.getByRole("menuitem", { name: /My notes/ });
  await expect(notes).toContainText("Needs a key or a sign-in.");
  await expect(menu.getByRole("menuitemcheckbox", { name: /My notes/ })).toHaveCount(0);

  await search.click();
  await expect(page.getByRole("list", { name: "Attached tools" })).toContainText("My search");
  await page.keyboard.press("Escape");

  // the chat that is started has it attached
  await page.getByLabel("Message", { exact: true }).fill("echo hi");
  await page.getByRole("button", { name: "Send" }).click();
  await expect(page).toHaveURL(THREAD_URL);
  const id = new URL(page.url()).pathname.split("/").pop();
  await expect
    .poll(async () => {
      const thread = (await (await mine.call("GET", `/api/threads/${id}`)).json()) as {
        tools?: string[];
      };
      return thread.tools ?? [];
    })
    .toEqual([ready.id]);

  // the one that needs setup is a link: it goes to the page where it is given what it needs
  await page.getByRole("button", { name: "Tools", exact: true }).click();
  await page.getByRole("menuitem", { name: /My notes/ }).click();
  await expect(page).toHaveURL(`${ORIGIN}/tools`);
});

test("no menu item without the permission, and the item with it", async ({
  page,
  mine,
  isMobile,
}) => {
  const openAccount = async () => {
    await page.goto("/");
    if (isMobile) {
      await page.getByRole("button", { name: "Threads", exact: true }).click();
      await expect(page.getByRole("dialog", { name: "Threads" })).toBeVisible();
    }
    await page.getByRole("button", { name: /^Account: / }).click();
  };
  await mine.as("user");
  await openAccount();
  await expect(page.getByRole("menuitem", { name: "Usage" })).toBeVisible();
  await expect(page.getByRole("menuitem", { name: "My tool servers" })).toHaveCount(0);

  await mine.as("tool-maker");
  await openAccount();
  await page.getByRole("menuitem", { name: "My tool servers" }).click();
  await expect(page).toHaveURL(`${ORIGIN}/tools`);
  await expect(page.getByRole("heading", { level: 1, name: "My tool servers" })).toBeVisible();
});

test("the page without the permission says so, and what the person added can still be deleted", async ({
  page,
  mine,
}) => {
  await mine.seed(bearer("Search"));
  // the permission is taken away: the server is not available to them, and the page is opened by its address
  await mine.as("user");
  await page.goto("/tools");
  await expect(
    page.getByText(/Your roles do not let you add or change tool servers of your own/),
  ).toBeVisible();
  await expect(page.getByRole("button", { name: "Add a server" })).toHaveCount(0);
  await expect(page.getByLabel("Address")).toHaveCount(0);
  const row = rowOf(page, "Search");
  await expect(row.locator('[data-slot="status"]')).toContainText("Not available.");
  await expect(row.locator('[data-slot="status"]')).toContainText(
    "Your roles no longer let you use servers of your own",
  );
  await expect(row.getByRole("button", { name: "Edit Search" })).toHaveCount(0);
  await row.getByRole("button", { name: "Delete Search" }).click();
  await page.getByRole("alertdialog").getByRole("button", { name: "Delete", exact: true }).click();
  await expect(rowOf(page, "Search")).toHaveCount(0);
});

test("a deployment that has the feature off says so, and a person with no roles is told that", async ({
  page,
  mine,
}) => {
  await mine.rules({ enabled: false });
  await page.goto("/tools");
  await expect(page.getByText(/does not let people add tool servers of their own/)).toBeVisible();
  await expect(page.getByLabel("Address")).toHaveCount(0);
  await mine.rules({ enabled: true });
  await mine.as("no-access");
  await page.reload();
  await expect(page.getByText(/Your roles grant no access to this app/)).toBeVisible();
});

test("on a phone the rows and the form need no sideways scroll, with a long name and a long host", async ({
  page,
  mine,
}) => {
  await mine.seed({
    name: "A server with a long name ".repeat(3).trim(),
    url: "https://a-very-long-host-name-that-keeps-going.subdomain.example.com/mcp",
    auth: { kind: "bearer" },
    secret: SECRET,
  });
  await mine.seed(oauth("Notes"));
  await page.goto("/tools");
  await expect(page.locator('[data-slot="server-row"]')).toHaveCount(2);
  await page.getByRole("button", { name: "Add a server" }).click();
  await expectNoHorizontalScroll(page);
});

for (const scheme of ["light", "dark"] as const) {
  test.describe(`accessibility (${scheme})`, () => {
    test.use({ colorScheme: scheme, contextOptions: { reducedMotion: "reduce" } });

    async function serious(page: Page) {
      await animationsDone(page);
      const results = await new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa"]).analyze();
      return results.violations.filter((v) => v.impact === "serious" || v.impact === "critical");
    }

    test("axe: the list in every state, the form, the dialogs and the page without the permission have no serious violations", async ({
      page,
      mine,
    }) => {
      await mine.seed(bearer("Search"));
      await mine.seed(oauth("Notes"));
      await mine.seed(bearer("Wiki", "wiki.example.com"));
      await mine.rules({ mark: { name: "Wiki", state: "revoked" } });
      await mine.seed(bearer("Inside", "inside.example.com"));
      await mine.rules({ denyHosts: ["inside.example.com"] });
      await page.goto("/tools");
      await expect(page.locator('[data-slot="server-row"]')).toHaveCount(4);
      expect(await serious(page)).toEqual([]);
      // the form, open
      await page.getByRole("button", { name: "Add a server" }).click();
      await page.getByLabel("Authentication").selectOption("api_key");
      expect(await serious(page)).toEqual([]);
      // the edit form with its warning
      await page.getByRole("button", { name: "Cancel" }).click();
      await rowOf(page, "Search").getByRole("button", { name: "Edit Search" }).click();
      await page.getByLabel("Address").fill("https://moved.example.com/mcp");
      expect(await serious(page)).toEqual([]);
      await page.reload();
      // the connect dialog
      await rowOf(page, "Notes").getByRole("button", { name: "Connect Notes" }).click();
      await expect(page.getByRole("alertdialog")).toContainText("Connect “Notes”?");
      expect(await serious(page)).toEqual([]);
      await page.getByRole("button", { name: "Cancel" }).click();
      // the delete dialog
      await rowOf(page, "Search").getByRole("button", { name: "Delete Search" }).click();
      await expect(page.getByRole("alertdialog")).toContainText("Delete “Search”?");
      expect(await serious(page)).toEqual([]);
      await page.getByRole("button", { name: "Cancel" }).click();
      // without the permission
      await mine.as("user");
      await page.reload();
      await expect(page.getByText(/Your roles do not let you add or change/)).toBeVisible();
      expect(await serious(page)).toEqual([]);
    });

    test("axe: the picker with a person's own servers has no serious violations", async ({
      page,
      mine,
    }) => {
      await mine.seed(bearer("My search"));
      await mine.seed(oauth("My notes"));
      await page.goto("/");
      await page.getByRole("button", { name: "Tools", exact: true }).click();
      await expect(page.getByRole("menu")).toContainText("Yours");
      expect(await serious(page)).toEqual([]);
    });
  });
}
