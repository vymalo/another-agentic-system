// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { SendSplit } from "./send-split";

afterEach(cleanup);
globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
};

const show = (props: Partial<Parameters<typeof SendSplit>[0]> = {}) => {
  const onSend = vi.fn();
  render(
    <SendSplit
      agent="planner"
      steers
      loading={false}
      onSend={onSend}
      onClosed={() => {}}
      {...props}
    />,
  );
  return onSend;
};

describe("SendSplit", () => {
  it("offers Send and a menu with Stop and send by default", async () => {
    const onSend = show();
    const more = screen.getByRole("button", { name: "Delivery options" });
    more.focus();
    fireEvent.keyDown(more, { key: "Enter" });
    expect(await screen.findByRole("menuitem", { name: /Stop and send/ })).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    expect(onSend).toHaveBeenCalledWith("steer");
  });

  it("is only Send when Stop and send is not offered: no menu, no menu item", () => {
    const onSend = show({ interrupt: false });
    expect(screen.queryByRole("button", { name: "Delivery options" })).toBeNull();
    expect(screen.queryByText(/Stop and send/)).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    expect(onSend).toHaveBeenCalledWith("steer");
  });

  it("rounds the end of Send when it stands alone, and squares it against the menu button otherwise", () => {
    show({ interrupt: false });
    expect(screen.getByRole("button", { name: "Send" }).className).toContain("rounded-e-full");
    expect(screen.getByRole("button", { name: "Send" }).className).not.toContain("rounded-e-none");
    cleanup();
    show({});
    expect(screen.getByRole("button", { name: "Send" }).className).toContain("rounded-e-none");
  });
});
