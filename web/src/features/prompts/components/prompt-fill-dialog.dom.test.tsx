// @vitest-environment jsdom
import { cleanup, configure, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { StrictMode } from "react";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import type { ApiPrompt } from "@/lib/api/types";
import { PromptFillDialog } from "./prompt-fill-dialog";

/*
 * The form that fills a prompt's variables: the catalog's own questions, one submit to `renderPrompt`,
 * and the text put in the message box (never sent). A stale prompt offers the current version and
 * keeps the answers whose questions still exist; a refused answer says where.
 */

beforeAll(() => {
  configure({ asyncUtilTimeout: 10_000 });
  globalThis.ResizeObserver = class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
  Element.prototype.scrollTo = () => {};
  Element.prototype.scrollIntoView = () => {};
});
afterEach(cleanup);

const prompt = (over: Partial<ApiPrompt> = {}): ApiPrompt => ({
  id: "p-aaaaaaaaaaaa",
  name: "review",
  title: "Review a change",
  description: "Reviews a change for one concern",
  body: "Review for {{focus}} at depth {{depth}}.",
  variables: [
    {
      id: "focus",
      question: "What focus?",
      options: [
        { value: "security", label: "Security" },
        { value: "speed", label: "Speed" },
      ],
    },
    { id: "depth", question: "How deep?", type: "number", unit: "levels" },
  ],
  own: true,
  revision: "2",
  updatedAt: "2026-11-15T10:00:00Z",
  ...over,
});

const setup = (over: Partial<ApiPrompt> = {}) => {
  const fill = vi
    .fn()
    .mockResolvedValue({ ok: true, text: "Review for Security at depth 3.", revision: "2" });
  const loadCurrent = vi.fn().mockResolvedValue({ ok: true, value: prompt() });
  const onFilled = vi.fn();
  const onClose = vi.fn();
  render(
    <PromptFillDialog
      prompt={prompt(over)}
      fill={fill}
      loadCurrent={loadCurrent}
      onFilled={onFilled}
      onClose={onClose}
    />,
  );
  return { fill, loadCurrent, onFilled, onClose };
};

const use = () => screen.getByRole("button", { name: "Use prompt" });

describe("the fill form", () => {
  it("draws the prompt's questions, holds the button until they are answered, and fills in order", async () => {
    const { fill, onFilled } = setup();
    expect(document.activeElement).toBe(screen.getByRole("heading", { name: "Review a change" }));
    expect(screen.getByText("/review", { exact: false })).toBeTruthy();
    expect(screen.getByText(/Reviews a change for one concern/)).toBeTruthy();
    expect(use().getAttribute("aria-disabled")).toBe("true");
    expect(screen.getByText("2 questions are not answered yet.")).toBeTruthy();

    fireEvent.click(screen.getByRole("radio", { name: "Security" }));
    expect(use().getAttribute("aria-disabled")).toBe("true");
    expect(screen.getByText("1 question is not answered yet.")).toBeTruthy();
    fireEvent.change(screen.getByRole("textbox", { name: /How deep\?/ }), {
      target: { value: "3" },
    });
    expect(use().getAttribute("aria-disabled")).toBeNull();

    fireEvent.click(use());
    await waitFor(() => expect(fill).toHaveBeenCalledTimes(1));
    expect(fill).toHaveBeenCalledWith(
      "2",
      [
        { id: "focus", values: ["security"] },
        { id: "depth", values: ["3"] },
      ],
      expect.objectContaining({ questions: expect.any(Array) }),
    );
    await waitFor(() => expect(onFilled).toHaveBeenCalledWith("Review for Security at depth 3."));
  });

  it("starts from the defaults the prompt gives, and sends them untouched", async () => {
    const { fill } = setup({
      variables: [
        {
          id: "focus",
          question: "What focus?",
          options: [
            { value: "security", label: "Security" },
            { value: "speed", label: "Speed" },
          ],
          default: "speed",
        },
      ],
    });
    expect((screen.getByRole("radio", { name: "Speed" }) as HTMLInputElement).checked).toBe(true);
    expect(use().getAttribute("aria-disabled")).toBeNull();
    fireEvent.click(use());
    await waitFor(() => expect(fill).toHaveBeenCalled());
    expect(fill.mock.calls[0]?.[1]).toEqual([{ id: "focus", values: ["speed"] }]);
  });

  it("fills a prompt with no questions with answers: []", async () => {
    const { fill } = setup({ variables: [] });
    expect(screen.getByText(/This prompt asks no questions\./)).toBeTruthy();
    expect(use().getAttribute("aria-disabled")).toBeNull();
    fireEvent.click(use());
    await waitFor(() => expect(fill).toHaveBeenCalled());
    expect(fill.mock.calls[0]?.[1]).toEqual([]);
  });

  it("says a stale prompt and loads the current version, keeping the answers that still fit", async () => {
    const { fill, loadCurrent } = setup();
    fireEvent.click(screen.getByRole("radio", { name: "Security" }));
    fireEvent.change(screen.getByRole("textbox", { name: /How deep\?/ }), {
      target: { value: "3" },
    });
    fill.mockResolvedValueOnce({
      ok: false,
      failure: {
        message:
          "This prompt was changed elsewhere since you opened it. Your answers are still here: press Load the current version to fill it as it is now.",
        stale: true,
      },
    });
    fireEvent.click(use());
    await waitFor(() => expect(screen.getByRole("alert")).toBeTruthy());
    expect(screen.getByRole("alert").textContent).toContain("changed elsewhere");
    // never an overwrite: the answers stay, and the current version is a choice
    expect((screen.getByRole("radio", { name: "Security" }) as HTMLInputElement).checked).toBe(
      true,
    );
    expect((screen.getByRole("textbox", { name: /How deep\?/ }) as HTMLInputElement).value).toBe(
      "3",
    );

    loadCurrent.mockResolvedValueOnce({
      ok: true,
      value: prompt({
        title: "Review a change, again",
        revision: "3",
        variables: [
          {
            id: "focus",
            question: "What focus?",
            options: [
              { value: "security", label: "Security" },
              { value: "speed", label: "Speed" },
            ],
          },
          { id: "depth", question: "How deep?", type: "number", unit: "levels" },
          { id: "when", question: "When?", type: "text" },
        ],
      }),
    });
    fireEvent.click(screen.getByRole("button", { name: "Load the current version" }));
    await waitFor(() =>
      expect(screen.getByRole("heading", { name: "Review a change, again" })).toBeTruthy(),
    );
    expect(loadCurrent).toHaveBeenCalledTimes(1);
    // the answer to a question that still exists is kept; the new one is asked for
    expect((screen.getByRole("radio", { name: "Security" }) as HTMLInputElement).checked).toBe(
      true,
    );
    expect((screen.getByRole("textbox", { name: /How deep\?/ }) as HTMLInputElement).value).toBe(
      "3",
    );
    expect(screen.getByText("1 question is not answered yet.")).toBeTruthy();

    fireEvent.change(screen.getByRole("textbox", { name: /When\?/ }), {
      target: { value: "Friday" },
    });
    fireEvent.click(use());
    await waitFor(() => expect(fill).toHaveBeenCalledTimes(2));
    expect(fill.mock.calls[1]?.[0]).toBe("3");
    expect(fill.mock.calls[1]?.[1]).toEqual([
      { id: "focus", values: ["security"] },
      { id: "depth", values: ["3"] },
      { id: "when", values: [], text: "Friday" },
    ]);
  });

  it("puts a refused answer at its question, in the rule's words", async () => {
    const { fill } = setup();
    fireEvent.click(screen.getByRole("radio", { name: "Security" }));
    fireEvent.change(screen.getByRole("textbox", { name: /How deep\?/ }), {
      target: { value: "3" },
    });
    fill.mockResolvedValueOnce({
      ok: false,
      failure: {
        message: "The answer to “How deep?” is not accepted: it is below the least.",
        question: "depth",
      },
    });
    fireEvent.click(use());
    await waitFor(() => expect(screen.getByRole("alert").textContent).toContain("below the least"));
    await waitFor(() =>
      expect(document.activeElement).toBe(screen.getByRole("textbox", { name: /How deep\?/ })),
    );
  });

  it("says a prompt whose kind this client cannot draw is not offered", () => {
    setup({
      variables: [
        {
          id: "picture",
          question: "Which picture?",
          type: "images",
          options: [
            {
              value: "a",
              label: "A",
              image: { sha256: "a".repeat(64), alt: "A" },
            },
          ],
        },
      ] as never,
    });
    expect(screen.getByRole("alert").textContent).toContain("cannot draw");
    expect(screen.queryByRole("button", { name: "Use prompt" })).toBeNull();
    expect(screen.getAllByRole("button", { name: "Close" }).length).toBeGreaterThan(0);
  });

  it("fills under StrictMode, whose double effect must not drop the answer", async () => {
    const fill = vi.fn().mockResolvedValue({ ok: true, text: "Strict.", revision: "2" });
    const onFilled = vi.fn();
    render(
      <StrictMode>
        <PromptFillDialog
          prompt={prompt({ variables: [{ id: "focus", question: "What focus?", type: "text" }] })}
          fill={fill}
          loadCurrent={vi.fn()}
          onFilled={onFilled}
          onClose={vi.fn()}
        />
      </StrictMode>,
    );
    fireEvent.change(screen.getByRole("textbox", { name: "What focus?" }), {
      target: { value: "Security" },
    });
    fireEvent.click(use());
    await waitFor(() => expect(onFilled).toHaveBeenCalledWith("Strict."));
  });

  it("a gone prompt cannot be filled", async () => {
    const { fill, onClose } = setup();
    fireEvent.click(screen.getByRole("radio", { name: "Security" }));
    fireEvent.change(screen.getByRole("textbox", { name: /How deep\?/ }), {
      target: { value: "3" },
    });
    fill.mockResolvedValueOnce({
      ok: false,
      failure: {
        message: "This prompt no longer exists, or is no longer shared with you.",
        gone: true,
      },
    });
    fireEvent.click(use());
    await waitFor(() =>
      expect(screen.getByRole("alert").textContent).toContain("no longer exists"),
    );
    expect(use().getAttribute("aria-disabled")).toBe("true");
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(onClose).toHaveBeenCalledTimes(1);
  });
});
