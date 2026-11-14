// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { setChoice, THEME_KEY } from "@/lib/theme";
import { ThemeMenu, ThemeSync } from "./theme-menu";

// Radix measures its popper content; jsdom has no layout
globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
};

beforeEach(() => {
  localStorage.clear();
  setChoice("system");
});
afterEach(() => {
  cleanup();
  document.documentElement.removeAttribute("data-theme");
});

async function openMenu() {
  fireEvent.keyDown(screen.getByRole("button", { name: "Theme" }), { key: "Enter" });
  return screen.findByRole("group", { name: "Theme" });
}

describe("the theme menu", () => {
  it("is a group of three radio items named Theme, with the current one checked", async () => {
    render(<ThemeMenu />);
    const group = await openMenu();
    const items = screen.getAllByRole("menuitemradio");
    expect(items.map((i) => i.textContent)).toEqual(["System", "Light", "Dark"]);
    expect(group.contains(items[0] as HTMLElement)).toBe(true);
    expect(screen.getByRole("menuitemradio", { name: "System" }).getAttribute("aria-checked")).toBe(
      "true",
    );
    expect(screen.getByRole("menuitemradio", { name: "Dark" }).getAttribute("aria-checked")).toBe(
      "false",
    );
  });

  it("changes the page at once, remembers the choice on this device and stays open", async () => {
    render(
      <>
        <ThemeSync />
        <ThemeMenu />
      </>,
    );
    await openMenu();
    fireEvent.click(screen.getByRole("menuitemradio", { name: "Dark" }));
    await waitFor(() => expect(document.documentElement.dataset.theme).toBe("dark"));
    expect(localStorage.getItem(THEME_KEY)).toBe("dark");
    expect(screen.getByRole("menuitemradio", { name: "Dark" }).getAttribute("aria-checked")).toBe(
      "true",
    );
    fireEvent.click(screen.getByRole("menuitemradio", { name: "Light" }));
    await waitFor(() => expect(document.documentElement.dataset.theme).toBe("light"));
  });
});
