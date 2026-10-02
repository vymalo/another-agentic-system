// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { Source, SourceGroup } from "../lib/sources";
import { SourcesView } from "./sources-tab";

afterEach(cleanup);

const turn = (n: number) => ({ id: `turn-${n}`, number: n });
const branch: Source = {
  key: "branch:github.com/acme/demo#agent/fix",
  kind: "branch",
  title: "agent/fix · acme/demo",
  turns: [turn(1)],
};
const ci: Source = {
  key: "https://ci.example.com/runs/1",
  kind: "ci",
  title: "CI: ci/build — failure",
  detail: "GitHub",
  href: "https://ci.example.com/runs/1",
  passed: false,
  turns: [turn(2)],
};
const group = (id: SourceGroup["id"], label: string, ...items: Source[]): SourceGroup => ({
  id,
  label,
  items,
});

describe("SourcesView", () => {
  it("is an empty state when nothing was shared", () => {
    render(<SourcesView groups={[]} onShowTurn={() => {}} />);
    expect(screen.getByText("Nothing shared yet")).toBeTruthy();
    expect(screen.queryByRole("list")).toBeNull();
  });

  it("a section per group, named by its heading, its items a list", () => {
    render(
      <SourcesView
        groups={[group("code", "Pull requests & branches", branch), group("checks", "Checks", ci)]}
        onShowTurn={() => {}}
      />,
    );
    const code = screen.getByRole("region", { name: "Pull requests & branches" });
    expect(within(code).getAllByRole("listitem")).toHaveLength(1);
    expect(screen.getByRole("region", { name: "Checks" })).toBeTruthy();
  });

  it("a kept file: a link that opens it and a button that downloads it", () => {
    const file: Source = {
      key: `file:${"9".repeat(64)}`,
      kind: "file",
      title: "chart.png",
      detail: "2.0 KB · image/png",
      href: `/api/threads/t-1/artifacts/${"9".repeat(64)}`,
      downloadHref: `/api/threads/t-1/artifacts/${"9".repeat(64)}?download=1`,
      turns: [turn(1)],
    };
    render(<SourcesView groups={[group("files", "Files", file)]} onShowTurn={() => {}} />);
    const row = screen.getByText("File · 2.0 KB · image/png").closest("li") as HTMLElement;
    expect(
      within(row)
        .getByRole("link", { name: /^chart\.png/ })
        .getAttribute("href"),
    ).toBe(file.href);
    const download = within(row).getByRole("link", { name: "Download chart.png" });
    expect(download.getAttribute("href")).toBe(file.downloadHref);
    expect(download.hasAttribute("download")).toBe(true);
  });

  it("what has nowhere to go is text, not a link: a branch", () => {
    render(
      <SourcesView
        groups={[group("code", "Pull requests & branches", branch)]}
        onShowTurn={() => {}}
      />,
    );
    expect(screen.queryByRole("link")).toBeNull();
    const row = screen.getByText("agent/fix · acme/demo").closest("li") as HTMLElement;
    expect(within(row).getByText("Branch")).toBeTruthy();
  });

  it("a failed CI report says so in words, with an icon that is not the only carrier", () => {
    render(<SourcesView groups={[group("checks", "Checks", ci)]} onShowTurn={() => {}} />);
    const link = screen.getByRole("link", { name: /CI: ci\/build — failure/ });
    expect(link.getAttribute("href")).toBe("https://ci.example.com/runs/1");
    const row = link.closest("li") as HTMLElement;
    expect(within(row).getByText("CI · GitHub")).toBeTruthy();
    // the icon is decoration
    expect(row.querySelector("svg")?.closest("[aria-hidden='true']")).toBeTruthy();
  });

  it("a Turn button per citing turn names the turn and reports it; past three it says how many more", () => {
    const onShowTurn = vi.fn();
    const cited: Source = { ...ci, turns: [turn(1), turn(2), turn(4), turn(7), turn(9)] };
    render(<SourcesView groups={[group("checks", "Checks", cited)]} onShowTurn={onShowTurn} />);
    const buttons = screen.getAllByRole("button");
    expect(buttons.map((b) => b.textContent)).toEqual(["Turn 1", "Turn 2", "Turn 4"]);
    expect(screen.getByText("+2")).toBeTruthy();
    expect(buttons[2]?.getAttribute("aria-label")).toBe("Show turn 4 in the conversation");
    fireEvent.click(buttons[2] as HTMLElement);
    expect(onShowTurn).toHaveBeenCalledWith("turn-4");
  });

  it("draws agent text as text: markup in a title is not markup", () => {
    const hostile: Source = {
      key: "https://x.example/",
      kind: "link",
      title: "<img src=x onerror=alert(1)>",
      detail: "x.example",
      href: "https://x.example/",
      turns: [turn(1)],
    };
    const { container } = render(
      <SourcesView groups={[group("links", "Links", hostile)]} onShowTurn={() => {}} />,
    );
    expect(container.querySelector("img")).toBeNull();
    expect(screen.getByRole("link").textContent).toContain("<img src=x onerror=alert(1)>");
  });
});
