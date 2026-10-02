// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { MessageEditor } from "./message-editor";

afterEach(cleanup);

const open = (over: Partial<Parameters<typeof MessageEditor>[0]> = {}) => {
  const onSend = vi.fn();
  const onCancel = vi.fn();
  render(
    <MessageEditor
      initialText="echo first"
      busy={false}
      onSend={onSend}
      onCancel={onCancel}
      {...over}
    />,
  );
  const field = screen.getByRole("textbox", { name: "What you said" }) as HTMLTextAreaElement;
  return { field, onSend, onCancel };
};

describe("the inline editor of a message", () => {
  it("opens on the message with the focus in it and the caret at its end", () => {
    const { field } = open();
    expect(field.value).toBe("echo first");
    expect(document.activeElement).toBe(field);
    expect([field.selectionStart, field.selectionEnd]).toEqual([10, 10]);
  });

  it("says its keys, and the field is described by them", () => {
    const { field } = open();
    const hint = document.getElementById(field.getAttribute("aria-describedby") ?? "");
    expect(hint?.textContent).toBe("Esc cancels · Ctrl/⌘ Enter sends");
  });

  it("Escape cancels and sends nothing", () => {
    const { field, onSend, onCancel } = open();
    fireEvent.change(field, { target: { value: "echo changed" } });
    fireEvent.keyDown(field, { key: "Escape" });
    expect(onCancel).toHaveBeenCalledTimes(1);
    expect(onSend).not.toHaveBeenCalled();
  });

  it("Ctrl+Enter and ⌘+Enter send what was typed, as typed", () => {
    const { field, onSend } = open();
    fireEvent.change(field, { target: { value: "  echo changed\n" } });
    fireEvent.keyDown(field, { key: "Enter", ctrlKey: true });
    expect(onSend).toHaveBeenLastCalledWith("  echo changed\n");
    fireEvent.keyDown(field, { key: "Enter", metaKey: true });
    expect(onSend).toHaveBeenCalledTimes(2);
  });

  it("a plain Enter is a new line: it sends nothing", () => {
    const { field, onSend } = open();
    const notPrevented = fireEvent.keyDown(field, { key: "Enter" });
    expect(notPrevented).toBe(true);
    expect(onSend).not.toHaveBeenCalled();
  });

  it("an Enter that finishes an input method's composition is left alone", () => {
    const { field, onSend } = open();
    fireEvent.keyDown(field, { key: "Enter", ctrlKey: true, isComposing: true });
    fireEvent.keyDown(field, { key: "Escape", isComposing: true });
    expect(onSend).not.toHaveBeenCalled();
  });

  it("an empty message is not sent, by key or by button", () => {
    const { field, onSend } = open();
    fireEvent.change(field, { target: { value: " \n " } });
    fireEvent.keyDown(field, { key: "Enter", ctrlKey: true });
    expect(onSend).not.toHaveBeenCalled();
    expect((screen.getByRole("button", { name: "Send" }) as HTMLButtonElement).disabled).toBe(true);
  });

  it("the buttons do what the keys do", () => {
    const { onSend, onCancel } = open();
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    expect(onSend).toHaveBeenCalledWith("echo first");
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(onCancel).toHaveBeenCalledTimes(1);
  });

  it("while the branch is made, Send waits and says so, and a second send is not made", () => {
    const { field, onSend } = open({ busy: true });
    expect(field.readOnly).toBe(true);
    const send = screen.getByRole("button", { name: "Sending…" }) as HTMLButtonElement;
    expect(send.disabled).toBe(true);
    fireEvent.keyDown(field, { key: "Enter", ctrlKey: true });
    expect(onSend).not.toHaveBeenCalled();
  });
});
