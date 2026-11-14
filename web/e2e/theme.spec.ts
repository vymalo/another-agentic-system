import { expect, type Page, test } from "@playwright/test";
import { agentPicker, badge, conversation, startThread } from "./helpers";

/**
 * The theme the person chooses (web/README.md "Theme"; ADR 0011, Amendment 2026-11-14): System (the default, follows the
 * device), Light or Dark, kept on this device and put on `<html data-theme>` by a head script before the first paint
 * (`src/lib/theme.ts`; the page's policy allows the script by its hash, e2e/csp.spec.ts). Everything that draws reads
 * that attribute, never the device: the stylesheet, the highlighter's colours and the diagrams.
 */

const DARK_BACKGROUND = "rgb(20, 22, 20)"; // --background, dark
const LIGHT_BACKGROUND = "rgb(255, 255, 255)"; // --background, light

const theme = (page: Page) => page.locator("html").getAttribute("data-theme");
const background = (page: Page) =>
  page.evaluate(() => getComputedStyle(document.body).backgroundColor);

/** The theme group of the account menu, opened if it is not (on a phone, from the thread list's sheet). */
async function openThemeGroup(page: Page, isMobile: boolean) {
  const group = page.getByRole("group", { name: "Theme" });
  if (await group.isVisible()) return group;
  if (isMobile) {
    await page.getByRole("button", { name: "Threads", exact: true }).click();
    await expect(page.getByRole("dialog", { name: "Threads" })).toBeVisible();
  }
  await page.getByRole("button", { name: /^Account: / }).click();
  await expect(group).toBeVisible();
  return group;
}

async function choose(page: Page, isMobile: boolean, word: "System" | "Light" | "Dark") {
  await openThemeGroup(page, isMobile);
  await page.getByRole("menuitemradio", { name: word }).click();
}

/**
 * Records, from the first byte of the document, every change of `data-theme` and the moment `<body>` is inserted (the
 * first thing that can paint), in order. A head script that ran after the body, or a React render that corrected the
 * attribute later, shows here: the log of a page with no flash is exactly `["attr:dark", "body"]` and nothing after.
 */
async function recordTheme(page: Page) {
  await page.addInitScript(() => {
    const events: string[] = [];
    const w = window as unknown as { __theme: { events: string[]; atReady: string | null } };
    w.__theme = { events, atReady: null };
    new MutationObserver((records) => {
      for (const r of records) {
        if (r.type === "attributes") {
          events.push(`attr:${document.documentElement.getAttribute("data-theme")}`);
        } else if ([...r.addedNodes].some((n) => n.nodeName === "BODY")) {
          events.push("body");
        }
      }
    }).observe(document, {
      attributes: true,
      attributeFilter: ["data-theme"],
      childList: true,
      subtree: true,
    });
    document.addEventListener("DOMContentLoaded", () => {
      w.__theme.atReady = getComputedStyle(document.body).backgroundColor;
    });
  });
}

const recorded = (page: Page) =>
  page.evaluate(
    () => (window as unknown as { __theme: { events: string[]; atReady: string | null } }).__theme,
  );

test.describe("on a light device", () => {
  test.use({ colorScheme: "light" });

  test("follows the device by default, and live", async ({ page }) => {
    await page.goto("/");
    await expect(agentPicker(page)).toBeVisible();
    expect(await theme(page)).toBe("light");
    expect(await background(page)).toBe(LIGHT_BACKGROUND);
    await page.emulateMedia({ colorScheme: "dark" });
    await expect.poll(() => theme(page)).toBe("dark");
    expect(await background(page)).toBe(DARK_BACKGROUND);
    await page.emulateMedia({ colorScheme: "light" });
    await expect.poll(() => theme(page)).toBe("light");
  });

  test("dark chosen stays dark after a reload, with no light frame", async ({ page, isMobile }) => {
    await page.goto("/");
    await expect(agentPicker(page)).toBeVisible();
    await choose(page, isMobile, "Dark");
    await expect.poll(() => theme(page)).toBe("dark");
    expect(await background(page)).toBe(DARK_BACKGROUND);

    await recordTheme(page);
    await page.reload();
    await expect(agentPicker(page)).toBeVisible();
    const log = await recorded(page);
    // set once, before the body existed, and never corrected afterwards (by React, say)
    expect(log.events.slice(0, 2)).toEqual(["attr:dark", "body"]);
    expect(log.events.filter((e) => e.startsWith("attr:"))).toEqual(["attr:dark"]);
    // the stylesheet is in by DOMContentLoaded: the first frame that can show the page is already dark
    expect(log.atReady).toBe(DARK_BACKGROUND);
    expect(await theme(page)).toBe("dark");
  });

  test("System again follows the device", async ({ page, isMobile }) => {
    await page.goto("/");
    await expect(agentPicker(page)).toBeVisible();
    await choose(page, isMobile, "Dark");
    await expect.poll(() => theme(page)).toBe("dark");
    await page.emulateMedia({ colorScheme: "light" });
    // a chosen theme does not follow the device
    await page.waitForTimeout(200);
    expect(await theme(page)).toBe("dark");
    await choose(page, isMobile, "System");
    await expect.poll(() => theme(page)).toBe("light");
    expect(await page.evaluate(() => localStorage.getItem("chat.theme"))).toBe("system");
    await page.emulateMedia({ colorScheme: "dark" });
    await expect.poll(() => theme(page)).toBe("dark");
  });

  test("the control is a named group, operable by keyboard", async ({ page, isMobile }) => {
    await page.goto("/");
    await expect(agentPicker(page)).toBeVisible();
    if (isMobile) {
      await page.getByRole("button", { name: "Threads", exact: true }).click();
    }
    const account = page.getByRole("button", { name: /^Account: / });
    await account.focus();
    await page.keyboard.press("Enter");
    const group = page.getByRole("group", { name: "Theme" });
    await expect(group).toBeVisible();
    const items = group.getByRole("menuitemradio");
    await expect(items).toHaveText(["System", "Light", "Dark"]);
    await expect(page.getByRole("menuitemradio", { name: "System" })).toBeChecked();

    // the arrow keys move between the three, and Enter chooses
    await page.getByRole("menuitemradio", { name: "System" }).focus();
    await page.keyboard.press("ArrowDown");
    await expect(page.getByRole("menuitemradio", { name: "Light" })).toBeFocused();
    await page.keyboard.press("ArrowDown");
    await expect(page.getByRole("menuitemradio", { name: "Dark" })).toBeFocused();
    await page.keyboard.press("Enter");
    await expect.poll(() => theme(page)).toBe("dark");
    await expect(page.getByRole("menuitemradio", { name: "Dark" })).toBeChecked();
    // the keyboard's place is kept: the chosen item still has the focus, so the person can go on with the arrows
    await expect(page.getByRole("menuitemradio", { name: "Dark" })).toBeFocused();
    // the menu stays, and nothing moved: the group is the same size as before
    await expect(group).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(group).toBeHidden();
  });

  test("the group is the same size whatever is chosen, so nothing moves", async ({
    page,
    isMobile,
  }) => {
    await page.goto("/");
    await expect(agentPicker(page)).toBeVisible();
    const group = await openThemeGroup(page, isMobile);
    const sizes: { width: number; height: number }[] = [];
    for (const word of ["System", "Light", "Dark"] as const) {
      await page.getByRole("menuitemradio", { name: word }).click();
      await expect(page.getByRole("menuitemradio", { name: word })).toBeChecked();
      const box = await group.boundingBox();
      sizes.push({ width: box?.width ?? 0, height: box?.height ?? 0 });
    }
    expect(sizes[1]).toEqual(sizes[0]);
    expect(sizes[2]).toEqual(sizes[0]);
  });

  test("a highlighted code block follows the chosen theme, not the device", async ({
    page,
    isMobile,
  }) => {
    await startThread(page, "code-math please", "Reviewer");
    await expect(badge(page)).toHaveText("Done");
    await expect(conversation(page).locator("pre code", { hasText: "fn total" })).toBeVisible();
    // by CSS from here on: an open menu hides the rest of the page from the accessibility tree, and from role locators
    const keyword = page
      .locator("pre code", { hasText: "fn total" })
      .locator("[data-token]", { hasText: "fn" })
      .first();
    await expect(keyword).toBeVisible();
    // the colours of the two themes the token carries, as the browser computes them
    const colours = () =>
      keyword.evaluate((el) => {
        const probe = (value: string) => {
          const span = document.createElement("span");
          span.style.color = value;
          document.body.append(span);
          const colour = getComputedStyle(span).color;
          span.remove();
          return colour;
        };
        return {
          actual: getComputedStyle(el).color,
          light: probe((el as HTMLElement).style.getPropertyValue("--shiki-light")),
          dark: probe((el as HTMLElement).style.getPropertyValue("--shiki-dark")),
        };
      });
    const onLight = await colours();
    expect(onLight.light).not.toBe(onLight.dark);
    expect(onLight.actual).toBe(onLight.light);
    await choose(page, isMobile, "Dark");
    await expect.poll(async () => (await colours()).actual).toBe(onLight.dark);
    await choose(page, isMobile, "Light");
    await expect.poll(async () => (await colours()).actual).toBe(onLight.light);
  });

  test("a diagram is drawn again in the chosen theme's colours", async ({ page, isMobile }) => {
    await startThread(page, "cards-mermaid please", "Reviewer");
    await expect(badge(page)).toHaveText("Done");
    const svg = async () => {
      const src = await page.locator('[data-slot="mermaid-image"]').getAttribute("src");
      return decodeURIComponent((src ?? "").split(",").slice(1).join(",")).toLowerCase();
    };
    await expect.poll(async () => (await svg()).includes("#f3f2ec")).toBe(true); // --muted, light
    await choose(page, isMobile, "Dark");
    await expect.poll(async () => (await svg()).includes("#232723")).toBe(true); // --muted, dark
    expect(await svg()).not.toContain("#f3f2ec");
  });
});

const BAR = { light: "#ffffff", dark: "#141614" }; // the `theme-color` of each theme (src/lib/theme.ts)

/** Whether every `theme-color` meta of the page holds `colour` (and there is at least one). */
const barIs = (page: Page, colour: string) =>
  page.evaluate((expected) => {
    const metas = [...document.querySelectorAll<HTMLMetaElement>('meta[name="theme-color"]')];
    return metas.length > 0 && metas.every((m) => m.content === expected);
  }, colour);

for (const [device, chosen] of [
  ["light", "dark"],
  ["dark", "light"],
] as const) {
  test.describe(`${chosen} stored on a ${device} device`, () => {
    test.use({ colorScheme: device });

    test("the address bar's colour is the chosen theme's, on both theme-color metas", async ({
      page,
    }) => {
      await page.addInitScript((value) => localStorage.setItem("chat.theme", value), chosen);
      await page.goto("/");
      await expect(agentPicker(page)).toBeVisible();
      await expect.poll(() => barIs(page, BAR[chosen])).toBe(true);
    });
  });
}

test.describe("the Theme button where there is no account menu", () => {
  test.use({ colorScheme: "light" });

  test("the sign-in screen has it: Dark is chosen, shown and kept after a reload", async ({
    page,
  }) => {
    // the mock is an edge deployment: the page is told it is a browser-mode one, whose issuer is never reached here
    await page.route("**/api/public/auth", (route) =>
      route.fulfill({
        json: {
          issuer: "https://auth.example.com/realms/acme",
          clientId: "web",
          scope: "openid email profile offline_access",
        },
      }),
    );
    await page.goto("/");
    await expect(page.locator('[data-slot="sign-in"]')).toBeVisible();
    await page.getByRole("button", { name: "Theme" }).click();
    await page.getByRole("menuitemradio", { name: "Dark" }).click();
    await expect.poll(() => theme(page)).toBe("dark");
    await page.reload();
    await expect(page.locator('[data-slot="sign-in"]')).toBeVisible();
    expect(await theme(page)).toBe("dark");
    await expect(page.getByRole("button", { name: "Theme" })).toBeVisible();
  });
});

test.describe("on a light device, in print", () => {
  test.use({ colorScheme: "light" });

  test("a dark theme prints with the light colours, and the screen is dark again after", async ({
    page,
  }) => {
    await page.addInitScript(() => localStorage.setItem("chat.theme", "dark"));
    await page.goto("/");
    await expect(agentPicker(page)).toBeVisible();
    const look = () =>
      page.evaluate(() => ({
        background: getComputedStyle(document.body).backgroundColor,
        text: getComputedStyle(document.body).color,
        scheme: getComputedStyle(document.documentElement).colorScheme,
      }));
    expect(await theme(page)).toBe("dark");
    expect(await look()).toEqual({
      background: DARK_BACKGROUND,
      text: "rgb(232, 233, 228)",
      scheme: "dark",
    });
    await page.emulateMedia({ media: "print" });
    // paper is white: the light tokens, though the page's theme is still dark
    expect(await theme(page)).toBe("dark");
    expect(await look()).toEqual({
      background: LIGHT_BACKGROUND,
      text: "rgb(26, 29, 26)",
      scheme: "light",
    });
    await page.emulateMedia({ media: "screen" });
    expect((await look()).background).toBe(DARK_BACKGROUND);
  });
});

test.describe("two tabs of one browser", () => {
  test.use({ colorScheme: "light" });

  test("a choice made in one is followed by the other, and so is a cleared storage", async ({
    context,
    page,
    isMobile,
  }) => {
    await page.goto("/");
    await expect(agentPicker(page)).toBeVisible();
    const other = await context.newPage();
    await other.goto("/");
    await expect(agentPicker(other)).toBeVisible();
    await choose(page, isMobile, "Dark");
    await expect.poll(() => theme(other)).toBe("dark");
    // `localStorage.clear()` in this tab tells the other tab with no key: it reads the choice again (System)
    await page.evaluate(() => localStorage.clear());
    await expect.poll(() => theme(other)).toBe("light");
  });
});

test.describe("the head script, with storage that throws from the first byte", () => {
  test.use({ colorScheme: "dark" });

  test("raises nothing, and has set the device's theme before the body exists", async ({
    page,
  }) => {
    const problems: string[] = [];
    page.on("pageerror", (e) => problems.push(`pageerror: ${e.message}`));
    page.on("console", (m) => {
      // a 404 of the mock for a resource is no script's error (it happens with storage working, too)
      if (m.type() === "error" && !/^Failed to load resource/.test(m.text()))
        problems.push(`console: ${m.text()}`);
    });
    await page.addInitScript(() => {
      Object.defineProperty(window, "localStorage", {
        configurable: true,
        get() {
          throw new DOMException("blocked", "SecurityError");
        },
      });
    });
    await recordTheme(page);
    await page.goto("/");
    await expect(agentPicker(page)).toBeVisible();
    const log = await recorded(page);
    expect(log.events.slice(0, 2)).toEqual(["attr:dark", "body"]);
    expect(problems).toEqual([]);
  });
});

test.describe("on a dark device", () => {
  test.use({ colorScheme: "dark" });

  test("light chosen stays light after a reload, with no dark frame", async ({
    page,
    isMobile,
  }) => {
    await page.goto("/");
    await expect(agentPicker(page)).toBeVisible();
    expect(await theme(page)).toBe("dark");
    await choose(page, isMobile, "Light");
    await expect.poll(() => theme(page)).toBe("light");
    expect(await background(page)).toBe(LIGHT_BACKGROUND);

    await recordTheme(page);
    await page.reload();
    await expect(agentPicker(page)).toBeVisible();
    const log = await recorded(page);
    expect(log.events.slice(0, 2)).toEqual(["attr:light", "body"]);
    expect(log.events.filter((e) => e.startsWith("attr:"))).toEqual(["attr:light"]);
    expect(log.atReady).toBe(LIGHT_BACKGROUND);
  });

  test("native controls follow the page: color-scheme is the chosen theme's", async ({
    page,
    isMobile,
  }) => {
    await page.goto("/");
    await expect(agentPicker(page)).toBeVisible();
    const scheme = () =>
      page.evaluate(() => getComputedStyle(document.documentElement).colorScheme);
    expect(await scheme()).toBe("dark");
    await choose(page, isMobile, "Light");
    await expect.poll(scheme).toBe("light");
  });

  test("storage refused: the page follows the system, and the control still works for the session", async ({
    page,
    isMobile,
  }) => {
    await page.addInitScript(() => {
      const refuse = () => {
        throw new DOMException("blocked", "SecurityError");
      };
      Storage.prototype.getItem = refuse;
      Storage.prototype.setItem = refuse;
      Storage.prototype.removeItem = refuse;
    });
    await page.goto("/");
    await expect(agentPicker(page)).toBeVisible();
    expect(await theme(page)).toBe("dark");
    await choose(page, isMobile, "Light");
    await expect.poll(() => theme(page)).toBe("light");
    await expect(page.getByRole("menuitemradio", { name: "Light" })).toBeChecked();
    await choose(page, isMobile, "System");
    await expect.poll(() => theme(page)).toBe("dark");
    // nothing was remembered: a reload follows the system
    await choose(page, isMobile, "Light");
    await expect.poll(() => theme(page)).toBe("light");
    await page.reload();
    await expect(agentPicker(page)).toBeVisible();
    expect(await theme(page)).toBe("dark");
  });
});
