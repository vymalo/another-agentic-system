import AxeBuilder from "@axe-core/playwright";
import { test as base, expect } from "@playwright/test";
import {
  agentMenu,
  agentMenuItem,
  agentPicker,
  animationsDone,
  closeAgentMenu,
  expectNoHorizontalScroll,
  openAgentMenu,
  THREAD_URL,
} from "./helpers";

/*
 * The platform's agent registry in the picker (ADR 0022): an agent the platform adds shows without
 * a reload, and a registry that cannot be read leaves the configured agents and says so.
 *
 * The mock keeps the registry per session, named by a cookie, so these tests change it without
 * showing their agents (or their outage) to the tests running beside them.
 */

const MOCK = "http://127.0.0.1:4010";
const ORIGIN = "http://127.0.0.1:3000";

type Registry = {
  /** The registry can (not) be read. */
  down: (down: boolean) => Promise<void>;
  /** The platform lists this agent from now on. */
  add: (agent: {
    id: string;
    name: string;
    description?: string;
    tags?: string[];
  }) => Promise<void>;
};

const test = base.extend<{ registry: Registry }>({
  registry: async ({ context, request }, use, info) => {
    const session = `e2e-${info.workerIndex}-${info.testId}`;
    await context.addCookies([{ name: "mock-registry", value: session, url: ORIGIN }]);
    const hook = (path: string, body?: unknown) =>
      request.post(
        `${MOCK}/__mock/registry${path}${path.includes("?") ? "&" : "?"}session=${session}`,
        {
          ...(body === undefined ? {} : { data: body }),
        },
      );
    await use({
      down: async (down) => {
        expect((await hook(`?down=${down}`)).ok()).toBe(true);
      },
      add: async (agent) => {
        expect((await hook("/agents", agent)).ok()).toBe(true);
      },
    });
  },
});

const NOTICE = "The agent registry is unreachable; showing the configured agents only.";

test("a registry that cannot be read is said on a new chat, with the configured agents still there", async ({
  page,
  registry,
}) => {
  await registry.down(true);
  await page.goto("/");
  await expect(agentPicker(page)).toHaveText("Agent: Coder · production");
  const notice = page.getByRole("status").filter({ hasText: NOTICE });
  await expect(notice).toBeVisible();
  await expectNoHorizontalScroll(page);

  // the configured agents are there to choose, and the menu says it too, with a Retry of its own
  const menu = await openAgentMenu(page);
  await expect(agentMenuItem(page, "Reviewer")).toBeVisible();
  await expect(menu.getByText(NOTICE)).toBeVisible();
  await expect(menu.getByRole("menuitem", { name: "Retry" })).toBeVisible();
  await closeAgentMenu(page);

  // the registry is back: Retry reads again, and the line goes
  await registry.down(false);
  await notice.getByRole("button", { name: "Retry" }).click();
  await expect(notice).toBeHidden();
  await expect(page.getByText(NOTICE)).toHaveCount(0);
});

test("the notice goes by itself when the picker is opened after the registry came back", async ({
  page,
  registry,
}) => {
  await registry.down(true);
  await page.goto("/");
  await expect(page.getByText(NOTICE)).toBeVisible();
  await registry.down(false);
  // opening the picker reads the list again
  await openAgentMenu(page);
  await expect(page.getByText(NOTICE)).toHaveCount(0);
});

test("an agent the platform adds is in the picker the next time it opens, without a reload", async ({
  page,
  registry,
}) => {
  let navigations = 0;
  page.on("framenavigated", () => {
    navigations += 1;
  });
  await page.goto("/");
  const agents = (menu: ReturnType<typeof agentMenu>) =>
    menu.getByRole("group", { name: "Agents" }).getByRole("menuitemradio");
  let menu = await openAgentMenu(page);
  await expect(agents(menu)).toHaveCount(3);
  await closeAgentMenu(page);
  const seen = navigations;

  await registry.add({
    id: "helper",
    name: "Helper",
    description: "Writes the docs.",
    tags: ["writing", "docs"],
  });
  menu = await openAgentMenu(page);
  await expect(agents(menu)).toHaveCount(4);
  const helper = agentMenuItem(page, "Helper");
  await expect(helper).toContainText("Writes the docs.");
  // the labels the platform keeps on it, as a hint
  await expect(helper).toContainText("writing · docs");
  expect(navigations, "the page was not reloaded").toBe(seen);
  // the configured agents come first: the default agent does not move
  await expect(agents(menu).first()).toContainText("Coder");
  await closeAgentMenu(page);
  await expect(agentPicker(page)).toHaveText("Agent: Coder · production");
});

test("a message goes to the agent the registry added, and the thread names it", async ({
  page,
  registry,
}) => {
  await registry.add({ id: "helper", name: "Helper", description: "Writes the docs." });
  await page.goto("/");
  await openAgentMenu(page);
  await agentMenuItem(page, "Helper").click();
  await expect(agentPicker(page)).toHaveText("Agent: Helper");
  await page.getByLabel("Message").fill("Write the README");
  const request = page.waitForRequest(
    (r) => r.method() === "POST" && new URL(r.url()).pathname === "/agui/agents/helper",
  );
  await page.getByRole("button", { name: "Send" }).click();
  await request;
  await expect(page).toHaveURL(THREAD_URL);
  await expect(agentPicker(page)).toHaveText("Agent: Helper");
});

for (const scheme of ["light", "dark"] as const) {
  test.describe(`accessibility with the registry unreachable (${scheme})`, () => {
    test.use({ colorScheme: scheme });

    const violations = async (page: import("@playwright/test").Page) => {
      await animationsDone(page);
      const results = await new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa"]).analyze();
      return results.violations.filter((v) => v.impact === "serious" || v.impact === "critical");
    };

    test("axe: the new chat with the notice, and the menu with its notice, have no serious violations", async ({
      page,
      registry,
    }) => {
      await registry.down(true);
      await page.goto("/");
      await expect(page.getByText(NOTICE)).toBeVisible();
      expect(await violations(page)).toEqual([]);
      await openAgentMenu(page);
      await expect(agentMenu(page).getByText(NOTICE)).toBeVisible();
      expect(await violations(page)).toEqual([]);
    });
  });
}
