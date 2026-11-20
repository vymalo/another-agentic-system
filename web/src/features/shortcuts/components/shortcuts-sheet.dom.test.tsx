// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { TooltipProvider } from "@/components/ui/tooltip";
import { SHORTCUTS } from "../lib/shortcuts";
import { ShortcutsSheet } from "./shortcuts-sheet";

/*
 * The help sheet (ADR 0067 S1): a modal dialog of the keys the page listens to, opened by `?` and by Ctrl/⌘+`/`, and by
 * nothing else. `?` is a character in a box, so it is not read there; Esc closes the sheet and the focus goes back where
 * it was. The rows are `SHORTCUTS`, the same array web/README.md quotes, and the modifiers are the platform's own (⌘ on
 * Apple, Ctrl elsewhere — jsdom is not an Apple platform).
 */

const show = () => {
  render(
    <TooltipProvider>
      <button type="button">before</button>
      <ShortcutsSheet />
      <textarea aria-label="Message" />
    </TooltipProvider>,
  );
};

const sheet = () => screen.queryByRole("dialog", { name: "Keyboard shortcuts" });
const open = () => expect(sheet()).not.toBeNull();

afterEach(cleanup);

describe("the keyboard shortcuts sheet", () => {
  it("opens with ? and lists every shortcut the page has, with the keys", () => {
    show();
    expect(sheet()).toBeNull();
    fireEvent.keyDown(document.body, { key: "?", shiftKey: true });
    open();
    for (const shortcut of SHORTCUTS) {
      expect(screen.getByText(shortcut.what)).toBeTruthy();
    }
    // the keys themselves: Ctrl (not ⌘) here, and the character of each chord
    expect(screen.getAllByText("Ctrl").length).toBeGreaterThan(0);
    expect(screen.queryByText("⌘")).toBeNull();
    expect(screen.getAllByText("Shift").length).toBeGreaterThan(0);
    // the key is a `KeyboardEvent.key`, the sheet draws what a keyboard shows
    expect(screen.getByText("Esc")).toBeTruthy();
    expect(screen.queryByText("Escape")).toBeNull();
  });

  it("lists its own two openers as one row, joined by or", () => {
    show();
    fireEvent.keyDown(document.body, { key: "?", shiftKey: true });
    const row = screen.getByText("Show these keys").nextElementSibling;
    expect(row?.textContent).toContain("or");
    expect(row?.querySelectorAll("kbd")).toHaveLength(3);
    expect(row?.textContent).toContain("?");
    expect(row?.textContent).toContain("/");
    // and it is the only row that offers two chords
    expect(screen.getAllByText("or")).toHaveLength(1);
  });

  it("is the sheet's own element, by slot", () => {
    show();
    fireEvent.keyDown(document.body, { key: "?", shiftKey: true });
    expect(document.querySelector('[data-slot="shortcuts-sheet"]')).toBe(sheet());
  });

  it("opens with Ctrl+/ and with ⌘+/", () => {
    show();
    fireEvent.keyDown(document.body, { key: "/", ctrlKey: true });
    open();
    cleanup();
    show();
    fireEvent.keyDown(document.body, { key: "/", metaKey: true });
    open();
  });

  it("opens with Ctrl+Shift+/ too, as an AZERTY keyboard sends it", () => {
    show();
    fireEvent.keyDown(document.body, { key: "/", ctrlKey: true, shiftKey: true });
    open();
  });

  it("is a modal dialog with a title, and Esc closes it", async () => {
    show();
    fireEvent.keyDown(document.body, { key: "?", shiftKey: true });
    const dialog = screen.getByRole("dialog", { name: "Keyboard shortcuts" });
    expect(dialog.getAttribute("aria-modal")).toBe("true");
    expect(screen.getByRole("heading", { name: "Keyboard shortcuts" })).toBeTruthy();
    fireEvent.keyDown(dialog, { key: "Escape" });
    await waitFor(() => expect(sheet()).toBeNull());
  });

  it("takes the focus and gives it back where it was", async () => {
    show();
    const before = screen.getByRole("button", { name: "before" });
    before.focus();
    fireEvent.keyDown(before, { key: "?", shiftKey: true });
    const dialog = screen.getByRole("dialog", { name: "Keyboard shortcuts" });
    await waitFor(() => expect(dialog.contains(document.activeElement)).toBe(true));
    fireEvent.keyDown(dialog, { key: "Escape" });
    await waitFor(() => expect(sheet()).toBeNull());
    await waitFor(() => expect(document.activeElement).toBe(before));
  });

  it("is not opened by ? while a person writes in a box", () => {
    show();
    const box = screen.getByLabelText("Message");
    fireEvent.keyDown(box, { key: "?", shiftKey: true });
    expect(sheet()).toBeNull();
  });

  it("is opened by Ctrl+/ from inside a box: no browser binds it", () => {
    show();
    fireEvent.keyDown(screen.getByLabelText("Message"), { key: "/", ctrlKey: true });
    open();
  });

  it("is not opened over a dialog that is already open", () => {
    show();
    const dialog = document.createElement("div");
    dialog.setAttribute("role", "dialog");
    dialog.setAttribute("data-state", "open");
    document.body.append(dialog);
    fireEvent.keyDown(document.body, { key: "?", shiftKey: true });
    expect(sheet()).toBeNull();
    fireEvent.keyDown(document.body, { key: "/", ctrlKey: true });
    expect(sheet()).toBeNull();
    dialog.remove();
  });

  it("is not opened by a key that is something else", () => {
    show();
    for (const event of [
      { key: "k", ctrlKey: true },
      { key: "/", shiftKey: true },
      { key: "?", altKey: true },
      { key: "?", ctrlKey: true },
    ]) {
      fireEvent.keyDown(document.body, event);
      expect(sheet()).toBeNull();
    }
  });
});
