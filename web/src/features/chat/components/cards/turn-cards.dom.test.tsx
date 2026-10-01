// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { FileCard, PullRequestCard } from "./turn-cards";

afterEach(cleanup);

describe("the pull request card", () => {
  it("shows the title, where it is and its branch, and opens it in a new tab", () => {
    render(
      <PullRequestCard
        pr={{
          href: "https://github.com/acme/demo/pull/12",
          label: "acme/demo#12",
          number: 12,
          repository: "acme/demo",
          branch: "agent/fix",
          title: "Fix the login",
        }}
      />,
    );
    expect(screen.getByText("Fix the login")).toBeTruthy();
    expect(screen.getByText("acme/demo#12")).toBeTruthy();
    expect(screen.getByText("agent/fix")).toBeTruthy();
    const link = screen.getByRole("link", {
      name: "View pull request acme/demo#12 (opens in a new tab)",
    });
    expect(link.getAttribute("href")).toBe("https://github.com/acme/demo/pull/12");
    expect(link.getAttribute("target")).toBe("_blank");
    expect(link.getAttribute("rel")).toBe("noopener noreferrer");
  });

  it("without a title it is the pull request by number, and the agent's note is text", () => {
    render(
      <PullRequestCard
        pr={{
          href: "https://github.com/acme/demo/pull/1",
          label: "acme/demo#1",
          number: 1,
          note: "<b>echo</b> **hi**",
        }}
      />,
    );
    expect(screen.getByText("Pull request #1")).toBeTruthy();
    expect(screen.getByText("<b>echo</b> **hi**")).toBeTruthy();
    expect(document.querySelector("b, strong")).toBeNull();
  });
});

describe("the file card", () => {
  it("names the file, shows its text as text, and folds a long one", () => {
    const text = Array.from({ length: 20 }, (_, i) => `line ${i}`).join("\n");
    render(<FileCard data={{ kind: "file", name: "notes.md", mimeType: "text/markdown", text }} />);
    expect(screen.getByText("notes.md")).toBeTruthy();
    expect(screen.getByText("text/markdown")).toBeTruthy();
    const pre = document.querySelector("pre") as HTMLElement;
    expect(pre.textContent).toContain("line 7");
    expect(pre.textContent).not.toContain("line 19");
    const more = screen.getByRole("button", { name: "Show all" });
    expect(more.getAttribute("aria-expanded")).toBe("false");
    fireEvent.click(more);
    expect(pre.textContent).toContain("line 19");
    expect(screen.getByRole("button", { name: "Show less" }).getAttribute("aria-expanded")).toBe(
      "true",
    );
  });

  it("links only an http(s) URI", () => {
    const { unmount } = render(
      <FileCard data={{ kind: "file", name: "report", uri: "https://example.com/r" }} />,
    );
    expect(screen.getByRole("link", { name: /Open report/ }).getAttribute("href")).toBe(
      "https://example.com/r",
    );
    unmount();
    render(<FileCard data={{ kind: "file", name: "report", uri: "javascript:alert(1)" }} />);
    expect(screen.queryByRole("link")).toBeNull();
  });
});
