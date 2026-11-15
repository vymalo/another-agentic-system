// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { toForm } from "../lib/editor";
import type { Failure } from "../lib/failures";
import type { PromptFields } from "../lib/library";
import { type FormOutcome, PromptForm } from "./prompt-form";

/*
 * The form of a prompt: what the server refused is told at the field its `at` names, with the focus there, the field marked and
 * tied to the words; a stale save keeps what was typed and offers the current version; what the page cannot edit is kept.
 */
globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
};

afterEach(cleanup);

const LIMITS = { maxVariables: 8, maxBodyBytes: 16384 };
const base: PromptFields = {
  title: "Review",
  name: "review",
  description: "",
  body: "Review for {{focus}} please.",
  questions: toForm([{ id: "focus", question: "Focus?", type: "text" }]),
};

const refusal = (over: Partial<Failure> & Pick<Failure, "message" | "target">): Failure => ({
  reread: false,
  ...over,
});

function mount(
  outcome: FormOutcome,
  extra: Partial<{ initial: PromptFields; onLoadCurrent: () => Promise<string | null> }> = {},
) {
  const onSubmit = vi.fn(async (_: PromptFields) => outcome);
  render(
    <PromptForm
      heading="Edit “Review”"
      mode="edit"
      initial={extra.initial ?? base}
      limits={LIMITS}
      onSubmit={onSubmit}
      onCancel={() => {}}
      {...(extra.onLoadCurrent ? { onLoadCurrent: extra.onLoadCurrent } : {})}
    />,
  );
  return onSubmit;
}

const save = () => fireEvent.click(screen.getByRole("button", { name: "Save changes" }));

describe("a refused save", () => {
  it("the refused field gets aria-invalid, the words and the focus", async () => {
    mount({
      ok: false,
      failure: refusal({ message: "A title is 1 to 120 characters.", target: { field: "title" } }),
    });
    save();
    const field = await screen.findByLabelText("Title");
    await waitFor(() => expect(document.activeElement).toBe(field));
    expect(field.getAttribute("aria-invalid")).toBe("true");
    const words = screen.getByText("A title is 1 to 120 characters.");
    expect(words.getAttribute("role")).toBe("alert");
    expect(field.getAttribute("aria-describedby")).toContain(words.id);
    // the other fields are not marked
    expect(screen.getByLabelText("Name").getAttribute("aria-invalid")).toBeNull();
  });

  it("bad_reference selects the braces at the offset, in the text", async () => {
    mount({
      ok: false,
      failure: refusal({
        message: "These braces do not start a blank.",
        target: { field: "body" },
        select: { start: 11, end: 13 },
      }),
    });
    save();
    const box = (await screen.findByLabelText("Text")) as HTMLTextAreaElement;
    await waitFor(() => expect(document.activeElement).toBe(box));
    expect(box.selectionStart).toBe(11);
    expect(box.selectionEnd).toBe(13);
    expect(box.value.slice(box.selectionStart, box.selectionEnd)).toBe("{{");
  });

  it("a refusal about a question puts the words in its card and the focus on the member it names", async () => {
    const initial: PromptFields = {
      ...base,
      questions: toForm([
        { id: "focus", question: "Focus?", type: "text" },
        { id: "depth", question: "Depth?", type: "number" },
      ]),
    };
    mount(
      {
        ok: false,
        failure: refusal({
          message: "Question 2: the largest value cannot be saved as it is.",
          target: { field: "question", index: 1, path: ["maxValue"] },
        }),
      },
      { initial },
    );
    save();
    const field = await screen.findByLabelText("Largest (optional)");
    await waitFor(() => expect(document.activeElement).toBe(field));
    expect(field.getAttribute("aria-invalid")).toBe("true");
    const card = field.closest('[data-slot="question-editor"]') as HTMLElement;
    expect(within(card).getByRole("alert").textContent).toContain("the largest value");
    // the first question is not marked
    expect(screen.getAllByRole("alert")).toHaveLength(1);
  });

  it("a refusal about a question's own card focuses the card when the member has no control", async () => {
    mount(
      {
        ok: false,
        failure: refusal({
          message: "This question is not used in the text.",
          target: { field: "question", index: 0, path: [] },
        }),
      },
      {},
    );
    save();
    const card = (await screen.findByRole("heading", { name: "Question 1" })) as HTMLElement;
    await waitFor(() => expect(document.activeElement).toBe(card));
    expect(card.getAttribute("aria-describedby")).toBeTruthy();
  });

  it("a stale save keeps the typed text and Load the current version asks the page for it", async () => {
    const onLoadCurrent = vi.fn(async () => null);
    mount(
      {
        ok: false,
        failure: refusal({
          message: "This prompt was changed elsewhere. Your changes are still here.",
          target: { field: "form" },
          stale: true,
          reread: true,
        }),
      },
      { onLoadCurrent },
    );
    fireEvent.change(screen.getByLabelText("Title"), { target: { value: "My typed title" } });
    save();
    await screen.findByText(/Your changes are still here/);
    expect((screen.getByLabelText("Title") as HTMLInputElement).value).toBe("My typed title");
    fireEvent.click(screen.getByRole("button", { name: "Load the current version" }));
    await waitFor(() => expect(onLoadCurrent).toHaveBeenCalledTimes(1));
  });

  it("says why the current version could not be loaded, and keeps the text", async () => {
    const onLoadCurrent = vi.fn(async () => "The prompt could not be loaded just now.");
    mount(
      {
        ok: false,
        failure: refusal({
          message: "Changed elsewhere.",
          target: { field: "form" },
          stale: true,
          reread: true,
        }),
      },
      { onLoadCurrent },
    );
    save();
    fireEvent.click(await screen.findByRole("button", { name: "Load the current version" }));
    await screen.findByText("The prompt could not be loaded just now.");
    expect((screen.getByLabelText("Title") as HTMLInputElement).value).toBe("Review");
  });
});

describe("the questions", () => {
  it("a question of a kind the page cannot edit is kept, said so, and sent as it was read", async () => {
    const scale = { id: "depth", question: "Depth?", type: "scale", to: 5 };
    const initial: PromptFields = {
      ...base,
      body: "{{focus}} {{depth}}",
      questions: toForm([{ id: "focus", question: "Focus?", type: "text" }, scale]),
    };
    const onSubmit = mount({ ok: true }, { initial });
    const kept = document.querySelector('[data-slot="kept-question"]') as HTMLElement;
    expect(kept.textContent).toContain("“Depth?” is a scale question");
    expect(kept.textContent).toContain("kept as it is");
    expect(within(kept).queryByRole("textbox")).toBeNull();
    save();
    await waitFor(() => expect(onSubmit).toHaveBeenCalled());
    const sent = onSubmit.mock.calls[0]?.[0] as PromptFields;
    expect(sent.questions[1]).toMatchObject({ kept: true, raw: scale });
  });

  it("Insert writes the name in braces where the caret was, and nowhere else", async () => {
    mount({ ok: true });
    const box = screen.getByLabelText("Text") as HTMLTextAreaElement;
    box.focus();
    box.setSelectionRange(7, 7);
    fireEvent.select(box);
    fireEvent.click(screen.getByRole("button", { name: "Insert {{focus}} into the text" }));
    await waitFor(() => expect(box.value).toBe("Review {{focus}}for {{focus}} please."));
  });

  it("adds a question, names it from its words, and stops at the most the configuration allows", async () => {
    mount({ ok: true }, { initial: { ...base, questions: [] } });
    fireEvent.click(screen.getByRole("button", { name: "Add a question" }));
    const words = screen.getByLabelText("Question");
    fireEvent.change(words, { target: { value: "Which environment should we use" } });
    expect((screen.getByLabelText("Name in the text") as HTMLInputElement).value).toBe(
      "which-environment-should",
    );
    for (let i = 0; i < 7; i++) {
      fireEvent.click(screen.getByRole("button", { name: "Add a question" }));
    }
    expect(screen.queryByRole("button", { name: "Add a question" })).toBeNull();
    expect(screen.getByText("A prompt has at most 8 questions.")).toBeTruthy();
  });

  it("questions are moved and removed with buttons that name the question", () => {
    mount(
      { ok: true },
      {
        initial: {
          ...base,
          questions: toForm([
            { id: "a", question: "Alpha?", type: "text" },
            { id: "b", question: "Beta?", type: "text" },
          ]),
        },
      },
    );
    fireEvent.click(screen.getByRole("button", { name: "Move question 2 up" }));
    const first = document.querySelector('[data-slot="question-editor"] input') as HTMLInputElement;
    expect(first.value).toBe("Beta?");
    fireEvent.click(screen.getByRole("button", { name: "Remove question 1" }));
    expect(screen.getAllByRole("heading", { name: /^Question \d$/ })).toHaveLength(1);
  });

  it("after a move the focus stays on the same control, or on the other direction's when that one is now disabled, and the new position is said", () => {
    mount(
      { ok: true },
      {
        initial: {
          ...base,
          questions: toForm([
            { id: "a", question: "Alpha?", type: "text" },
            { id: "b", question: "Beta?", type: "text" },
            { id: "c", question: "Gamma?", type: "text" },
          ]),
        },
      },
    );
    fireEvent.click(screen.getByRole("button", { name: "Move question 2 down" }));
    // Beta is last now: its Down is disabled, so the focus is on its Up
    expect(document.activeElement).toBe(screen.getByRole("button", { name: "Move question 3 up" }));
    expect(screen.getByText("Question moved to position 3 of 3").getAttribute("role")).toBe(
      "status",
    );
    fireEvent.click(screen.getByRole("button", { name: "Move question 3 up" }));
    // Beta is second: Up is still enabled and keeps the focus
    expect(document.activeElement).toBe(screen.getByRole("button", { name: "Move question 2 up" }));
    expect(screen.getByText("Question moved to position 2 of 3")).toBeTruthy();
  });

  it("an option moved keeps the focus on its control and says where it went", () => {
    mount(
      { ok: true },
      {
        initial: {
          ...base,
          questions: toForm([
            {
              id: "a",
              question: "Alpha?",
              type: "single",
              options: [
                { value: "x", label: "X" },
                { value: "y", label: "Y" },
              ],
            },
          ]),
        },
      },
    );
    fireEvent.click(screen.getByRole("button", { name: "Move option 1 of question 1 down" }));
    expect(document.activeElement).toBe(
      screen.getByRole("button", { name: "Move option 2 of question 1 up" }),
    );
    expect(screen.getByText("Option moved to position 2 of 2")).toBeTruthy();
  });

  it("the move and remove buttons are disabled while a save is pending", async () => {
    let finish: (o: FormOutcome) => void = () => {};
    const onSubmit = vi.fn(() => new Promise<FormOutcome>((r) => (finish = r)));
    render(
      <PromptForm
        heading="Edit"
        mode="edit"
        initial={{
          ...base,
          questions: toForm([
            { id: "a", question: "Alpha?", type: "text" },
            { id: "b", question: "Beta?", type: "text" },
          ]),
        }}
        limits={LIMITS}
        onSubmit={onSubmit}
        onCancel={() => {}}
      />,
    );
    save();
    await waitFor(() => expect(screen.getByRole("button", { name: "Saving…" })).toBeTruthy());
    for (const name of ["Move question 1 down", "Move question 2 up", "Remove question 1"]) {
      expect((screen.getByRole("button", { name }) as HTMLButtonElement).disabled, name).toBe(true);
    }
    finish({ ok: true });
  });

  it("a double submit calls onSubmit once", async () => {
    let finish: (o: FormOutcome) => void = () => {};
    const onSubmit = vi.fn(() => new Promise<FormOutcome>((r) => (finish = r)));
    render(
      <PromptForm
        heading="Edit"
        mode="edit"
        initial={base}
        limits={LIMITS}
        onSubmit={onSubmit}
        onCancel={() => {}}
      />,
    );
    const form = document.querySelector("form") as HTMLFormElement;
    fireEvent.submit(form);
    fireEvent.submit(form);
    await waitFor(() => expect(screen.getByRole("button", { name: "Saving…" })).toBeTruthy());
    fireEvent.submit(form);
    expect(onSubmit).toHaveBeenCalledTimes(1);
    finish({ ok: true });
  });

  it("a question has at most 12 options: at 12 Add option is gone, with its sentence", () => {
    mount(
      { ok: true },
      {
        initial: {
          ...base,
          questions: toForm([
            {
              id: "a",
              question: "Alpha?",
              type: "single",
              options: Array.from({ length: 11 }, (_, k) => ({ value: `v${k}`, label: `L${k}` })),
            },
          ]),
        },
      },
    );
    expect(screen.queryByText("A question has at most 12 options.")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Add an option to question 1" }));
    expect(screen.getAllByLabelText(/^Option \d+ of question 1$/)).toHaveLength(12);
    expect(screen.queryByRole("button", { name: "Add an option to question 1" })).toBeNull();
    expect(screen.getByText("A question has at most 12 options.")).toBeTruthy();
  });

  it("changing the kind of a question with settings the page does not show says they are removed", () => {
    mount(
      { ok: true },
      {
        initial: {
          ...base,
          questions: toForm([
            { id: "a", question: "Alpha?", type: "number", default: 3 },
            { id: "b", question: "Beta?", type: "text" },
          ]),
        },
      },
    );
    const kinds = screen.getAllByLabelText("Kind");
    const note = "Changing the kind removes the settings this page does not show.";
    expect(screen.queryByText(note)).toBeNull();
    fireEvent.change(kinds[1] as HTMLElement, { target: { value: "longText" } });
    expect(screen.queryByText(note)).toBeNull();
    fireEvent.change(kinds[0] as HTMLElement, { target: { value: "date" } });
    expect(screen.getByText(note)).toBeTruthy();
  });

  it("shows the size of the text in bytes against the configuration's limit", () => {
    mount({ ok: true }, { initial: { ...base, body: "é{{focus}}" } });
    expect(screen.getByText("11 of 16,384 bytes")).toBeTruthy();
  });
});
