// @vitest-environment jsdom
import { cleanup, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { ChecklistView } from "./checklist";

afterEach(cleanup);

describe("a dropped item", () => {
  const items = [
    { label: "Read the test", state: "done" },
    { label: "Port it", state: "dropped", note: "Out of scope" },
  ];

  it("says Dropped in words beside its icon, whatever the strike does", () => {
    render(<ChecklistView items={items} title="Plan" />);
    const row = document.querySelector('li[data-state="dropped"]') as HTMLElement;
    expect(within(row).getByText("Dropped")).toBeTruthy();
    expect(within(row).getByText("Out of scope")).toBeTruthy();
  });

  it("is struck through and in italics, and a done item is struck through too", () => {
    render(<ChecklistView items={items} title="Plan" />);
    const dropped = screen.getByText("Port it");
    expect(dropped.className).toContain("line-through");
    expect(dropped.className).toContain("italic");
    expect(screen.getByText("Read the test").className).toContain("line-through");
  });

  it("is counted in neither number", () => {
    render(<ChecklistView items={items} title="Plan" />);
    expect(document.querySelector('[data-slot="checklist-count"]')?.textContent).toBe(
      "1 of 1 done",
    );
    expect(screen.getByRole("progressbar").getAttribute("aria-valuemax")).toBe("1");
  });
});

describe("a checklist with nothing left to count", () => {
  it("draws no progress bar when every item is dropped, and says 0 of 0 done in words", () => {
    render(
      <ChecklistView
        items={[
          { label: "A", state: "dropped" },
          { label: "B", state: "dropped" },
        ]}
        title="Plan"
      />,
    );
    expect(screen.queryByRole("progressbar")).toBeNull();
    expect(document.querySelector('[data-slot="checklist-count"]')?.textContent).toBe(
      "0 of 0 done",
    );
    expect(screen.getAllByText("Dropped")).toHaveLength(2);
  });

  it("draws the bar again as soon as one item counts", () => {
    render(
      <ChecklistView
        items={[
          { label: "A", state: "dropped" },
          { label: "B", state: "todo" },
        ]}
        title="Plan"
      />,
    );
    expect(screen.getByRole("progressbar").getAttribute("aria-valuetext")).toBe("0 of 1 done");
  });
});
