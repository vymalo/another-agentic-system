// @vitest-environment jsdom
import { act, cleanup, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useScrollToMessage } from "./use-scroll-to-message";

/*
 * `/threads/<id>#m-<seq>`: a person's message is found by its id. A search hit in an agent's answer (ADR 0068) names an event that
 * has no element, so, once the transcript has been still, the page goes to the last person's message at or before it.
 */

const message = (seq: number) => {
  const el = document.createElement("div");
  el.id = `m-${seq}`;
  el.dataset.seq = String(seq);
  el.tabIndex = -1;
  el.scrollIntoView = vi.fn();
  document.body.append(el);
  return el;
};

beforeEach(() => {
  vi.useFakeTimers();
  vi.stubGlobal("requestAnimationFrame", (cb: FrameRequestCallback) => setTimeout(() => cb(0), 0));
  vi.stubGlobal("cancelAnimationFrame", (id: number) => clearTimeout(id));
});
afterEach(() => {
  cleanup();
  document.body.replaceChildren();
  window.location.hash = "";
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

const settle = (ms: number) => act(() => vi.advanceTimersByTimeAsync(ms));

describe("useScrollToMessage", () => {
  it("goes to the message the hash names", async () => {
    window.location.hash = "#m-5";
    const five = message(5);
    message(1);
    renderHook(() => useScrollToMessage("t", true));
    await settle(10);
    expect(five.scrollIntoView).toHaveBeenCalled();
    expect(document.activeElement).toBe(five);
  });

  it("falls back to the last person's message at or before an event that has none", async () => {
    window.location.hash = "#m-9";
    message(1);
    const three = message(3);
    const twelve = message(12);
    renderHook(() => useScrollToMessage("t", true));
    await settle(100);
    // not yet: the exact message might still be drawn
    expect(three.scrollIntoView).not.toHaveBeenCalled();
    await settle(700);
    expect(three.scrollIntoView).toHaveBeenCalled();
    expect(twelve.scrollIntoView).not.toHaveBeenCalled();
  });

  it("takes the exact message when it is drawn before the page has settled", async () => {
    window.location.hash = "#m-9";
    const one = message(1);
    renderHook(() => useScrollToMessage("t", true));
    await settle(300);
    const nine = message(9);
    await settle(10);
    expect(nine.scrollIntoView).toHaveBeenCalled();
    await settle(1_000);
    expect(one.scrollIntoView).not.toHaveBeenCalled();
  });

  it("goes again when the hash changes on the open page", async () => {
    const one = message(1);
    const four = message(4);
    renderHook(() => useScrollToMessage("t", true));
    await settle(10);
    expect(one.scrollIntoView).not.toHaveBeenCalled();
    window.location.hash = "#m-4";
    await settle(10);
    expect(four.scrollIntoView).toHaveBeenCalled();
  });

  it("waits until the transcript has been still for 600 ms: every piece drawn starts the wait again (7a)", async () => {
    window.location.hash = "#m-9";
    message(1);
    renderHook(() => useScrollToMessage("t", true));
    await settle(500);
    const three = message(3);
    await settle(500);
    // 1000 ms after the mount, 500 after the last piece: not still yet
    expect(three.scrollIntoView).not.toHaveBeenCalled();
    const four = message(4);
    await settle(700);
    expect(four.scrollIntoView).toHaveBeenCalled();
    expect(three.scrollIntoView).not.toHaveBeenCalled();
  });

  it("gives up after ten seconds: a message drawn later is not gone to (7b)", async () => {
    window.location.hash = "#m-9";
    renderHook(() => useScrollToMessage("t", true));
    await settle(11_000);
    const three = message(3);
    await settle(1_000);
    expect(three.scrollIntoView).not.toHaveBeenCalled();
  });

  it("moves to the exact message when it is drawn after the page settled for an earlier one (7c)", async () => {
    window.location.hash = "#m-9";
    const three = message(3);
    renderHook(() => useScrollToMessage("t", true));
    await settle(800);
    expect(three.scrollIntoView).toHaveBeenCalledTimes(1);
    const nine = message(9);
    await settle(100);
    expect(nine.scrollIntoView).toHaveBeenCalled();
    expect(document.activeElement).toBe(nine);
  });

  it("scrolls but keeps the focus of a person who is typing in another field (7c)", async () => {
    const field = document.createElement("textarea");
    document.body.append(field);
    field.focus();
    window.location.hash = "#m-5";
    const five = message(5);
    renderHook(() => useScrollToMessage("t", true));
    await settle(50);
    expect(five.scrollIntoView).toHaveBeenCalled();
    expect(document.activeElement).toBe(field);
  });

  it("takes the focus from the search box: a hit chosen there is meant to", async () => {
    const box = document.createElement("input");
    box.dataset.slot = "search-input";
    document.body.append(box);
    box.focus();
    window.location.hash = "#m-5";
    const five = message(5);
    renderHook(() => useScrollToMessage("t", true));
    await settle(50);
    expect(document.activeElement).toBe(five);
  });

  it("does nothing without a hash", async () => {
    const one = message(1);
    renderHook(() => useScrollToMessage("t", true));
    await settle(2_000);
    expect(one.scrollIntoView).not.toHaveBeenCalled();
  });
});
