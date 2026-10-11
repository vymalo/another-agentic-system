// @vitest-environment jsdom
import { cleanup, fireEvent, render } from "@testing-library/react";
import { useState } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { TurnSteps } from "@/features/chat/lib/step-tree";
import { ToolServersProvider } from "@/features/tools/components/tool-servers-context";
import type { ApiToolServer } from "@/lib/api/types";
import { type ExpansionState, NO_EXPANSION } from "./expansion";
import { StepsPane } from "./steps-pane";
import { actorPart, assistant, RUNNING_VIEW, statusPart, stepPart, turnsOf } from "./testing";

/*
 * A step of an MCP server the page knows by name (ADR 0049, amended 2026-10-11): `github__list_commits`
 * reads "List commits" beside the GitHub mark, not the name an agent calls the tool by. That name
 * stays where a person debugging looks for it: the tooltip of the words, and the step once opened.
 */

beforeEach(() => {
  Element.prototype.scrollIntoView = vi.fn();
  globalThis.ResizeObserver ??= class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
});
afterEach(cleanup);

const SVG = "data:image/svg+xml;base64,PHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciLz4=";

function Harness({
  turns,
  servers = [],
}: {
  turns: readonly TurnSteps[];
  servers?: readonly ApiToolServer[];
}) {
  const [expanded, setExpanded] = useState<ExpansionState>(NO_EXPANSION);
  return (
    <ToolServersProvider servers={servers}>
      <StepsPane
        turns={turns}
        focus={null}
        live={false}
        expanded={expanded}
        onExpandedChange={setExpanded}
      />
    </ToolServersProvider>
  );
}

const call = (
  id: string,
  label: string,
  state = "completed",
  extra: Record<string, unknown> = {},
) =>
  stepPart(
    id,
    state,
    {
      label,
      input: { owner: "vymalo", repo: "panda-tinder", per_page: 3 },
      output: { text: "a1b2c3d Add the swipe deck" },
      ...extra,
    },
    2,
  );
const turnOf = (...steps: ReturnType<typeof stepPart>[]): TurnSteps[] =>
  turnsOf([assistant([actorPart(), statusPart("working", undefined, 1), ...steps])]);
const runningTurnOf = (...steps: ReturnType<typeof stepPart>[]): TurnSteps[] =>
  turnsOf(
    [assistant([actorPart(), statusPart("working", undefined, 1), ...steps], { type: "running" })],
    RUNNING_VIEW,
  );

const rowOf = (id: string): HTMLElement =>
  document.querySelector(`[data-step="${id}"]`) as HTMLElement;
const toggleOf = (id: string): HTMLElement =>
  rowOf(id).querySelector('[data-slot="step-toggle"]') as HTMLElement;
const markOf = (id: string, brand: string) => rowOf(id).querySelector(`svg[data-brand="${brand}"]`);

describe("a GitHub tool step", () => {
  it("reads as the action, beside the GitHub mark, and not as the tool's name", () => {
    render(<Harness turns={turnOf(call("T/a", "github__list_commits"))} />);
    const row = rowOf("T/a");
    expect(markOf("T/a", "github")).not.toBeNull();
    expect(toggleOf("T/a").textContent).toContain("List commits");
    expect(toggleOf("T/a").textContent).not.toContain("github__list_commits");
    // the mark says GitHub, so the server's name is not said again as a chip
    expect(row.querySelector('[data-slot="step-server"]')).toBeNull();
  });

  it("says GitHub to a screen reader and in the tooltip of the mark", () => {
    render(<Harness turns={turnOf(call("T/a", "github__get_file_contents"))} />);
    const row = rowOf("T/a");
    expect(row.querySelector('[title="GitHub"]')).not.toBeNull();
    expect(row.querySelector(".sr-only")?.textContent).toBe("GitHub: ");
    expect(toggleOf("T/a").textContent).toContain("Get file contents");
  });

  it("keeps the name an agent calls the tool by in the tooltip, and in the step once it is opened", () => {
    render(<Harness turns={turnOf(call("T/a", "github__list_commits"))} />);
    expect(toggleOf("T/a").querySelector("span[title]")?.getAttribute("title")).toBe(
      "github__list_commits",
    );
    expect(rowOf("T/a").querySelector('[data-slot="step-tool"]')).toBeNull();
    fireEvent.click(toggleOf("T/a"));
    const tool = rowOf("T/a").querySelector('[data-slot="step-tool"]') as HTMLElement;
    expect(tool.textContent).toContain("github__list_commits");
    expect(tool.querySelector("code")?.textContent).toBe("github__list_commits");
  });

  it("keeps the mark neutral in every state: a brand's mark is not red when the call failed", () => {
    render(
      <Harness
        turns={turnOf(
          call("T/a", "github__list_commits"),
          call("T/b", "github__get_commit", "failed"),
        )}
      />,
    );
    for (const id of ["T/a", "T/b"]) {
      const cls = markOf(id, "github")?.getAttribute("class") ?? "";
      expect(cls, id).toContain("text-foreground");
    }
    expect(rowOf("T/b").getAttribute("data-node-state")).toBe("failed");
  });

  it("is the spinner while the call runs, like every step", () => {
    render(<Harness turns={runningTurnOf(call("T/a", "github__list_commits", "running"))} />);
    expect(markOf("T/a", "github")).toBeNull();
  });

  it("gives way to the image a deployment gave the server", () => {
    render(
      <Harness
        turns={turnOf(
          call("T/a", "github__list_commits", "completed", { icon: "mcp-server:github" }),
        )}
        servers={[{ id: "github", name: "GitHub", icon: SVG }]}
      />,
    );
    expect(rowOf("T/a").querySelectorAll("img")).toHaveLength(1);
    expect(markOf("T/a", "github")).toBeNull();
    expect(toggleOf("T/a").textContent).toContain("List commits");
  });

  it("is a command like any other when the step is not a tool", () => {
    render(
      <Harness
        turns={turnOf(call("T/a", "github__list_commits", "completed", { kind: "command" }))}
      />,
    );
    expect(markOf("T/a", "github")).toBeNull();
  });
});

describe("a tool step of another server", () => {
  it("is called by its action, with the server named and the glyph of a tool, when the page does not know it", () => {
    render(<Harness turns={turnOf(call("T/a", "jira__create_issue"))} />);
    expect(toggleOf("T/a").textContent).toContain("Create issue");
    expect(rowOf("T/a").querySelector('[data-slot="step-server"]')?.textContent).toContain("jira");
    expect(rowOf("T/a").querySelector("svg[data-brand]")).toBeNull();
  });

  it("has the glyph of a server the page knows, and keeps the server's name beside it", () => {
    render(
      <Harness
        turns={turnOf(
          call("T/a", "browser__browser_navigate"),
          call("T/b", "context7__resolve_library_id"),
          call("T/c", "websearch__web_search"),
        )}
      />,
    );
    expect(toggleOf("T/a").textContent).toContain("Browser navigate");
    expect(rowOf("T/a").querySelector('[title="Browser"]')).not.toBeNull();
    expect(rowOf("T/b").querySelector('[title="Context7"]')).not.toBeNull();
    expect(rowOf("T/c").querySelector('[title="Web search"]')).not.toBeNull();
    for (const id of ["T/a", "T/b", "T/c"]) {
      // a glyph is not a logo: the name stays as the chip, and there is no brand mark
      expect(rowOf(id).querySelector('[data-slot="step-server"]'), id).not.toBeNull();
      expect(rowOf(id).querySelector("svg[data-brand]"), id).toBeNull();
    }
  });

  it("keeps the icon an agent chose over the glyph of the server", () => {
    render(
      <Harness
        turns={turnOf(call("T/a", "browser__browser_screenshot", "completed", { icon: "web" }))}
      />,
    );
    expect(rowOf("T/a").querySelector('[title="Browser"]')).toBeNull();
  });
});
