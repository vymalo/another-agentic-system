// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { AgentCapabilities } from "@/features/agents/hooks/use-agent-capabilities";
import type { ApiToolServer } from "@/lib/api/types";
import type { ToolServersView } from "../hooks/use-tool-servers";
import { THREAD_TOOLS_URI } from "../lib/servers";
import { ToolsPicker, ToolsWarning } from "./tools-picker";

afterEach(cleanup);

// Radix measures its popper content; jsdom has no layout
globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
};

const SVG = "data:image/svg+xml;base64,PHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciLz4=";

const servers: ApiToolServer[] = [
  { id: "websearch", name: "Web search", description: "Search the web.", icon: SVG },
  { id: "github", name: "GitHub", agents: ["coder"] },
  { id: "docs", name: "Team docs", icon: "https://tracker.example/docs.png" },
];

const view = (over: Partial<ToolServersView> = {}): ToolServersView => ({
  servers,
  loading: false,
  unavailable: false,
  error: null,
  reload: vi.fn(),
  ...over,
});

const caps = (listed: boolean | "unreadable" | "unknown" = true): AgentCapabilities => {
  const refresh = vi.fn();
  if (listed === "unreadable") return { status: "unreadable", supports: () => null, refresh };
  if (listed === "unknown") return { status: "unknown", supports: () => null, refresh };
  return { status: "ready", supports: (uri) => listed && uri === THREAD_TOOLS_URI, refresh };
};

function renderPicker(over: Partial<Parameters<typeof ToolsPicker>[0]> = {}) {
  const onChange = vi.fn<(next: string[]) => void>();
  const utils = render(
    <ToolsPicker
      view={view()}
      agentId="coder"
      chosen={[]}
      onChange={onChange}
      capabilities={caps()}
      {...over}
    />,
  );
  return { ...utils, onChange };
}

const trigger = () => screen.getByRole("button", { name: "Tools" });
function open() {
  trigger().focus();
  fireEvent.keyDown(trigger(), { key: "Enter" });
  return screen.findByRole("menu");
}

describe("ToolsPicker", () => {
  it("is a menu button that lists the servers offered for the agent, each with its name and what it is for", async () => {
    renderPicker();
    expect(trigger().getAttribute("aria-haspopup")).toBe("menu");
    const menu = await open();
    const items = within(menu).getAllByRole("menuitemcheckbox");
    expect(items.map((i) => i.textContent)).toEqual([
      "Web searchSearch the web.",
      "GitHub",
      "Team docs",
    ]);
    for (const i of items) expect(i.getAttribute("aria-checked")).toBe("false");
  });

  it("offers a server only for the agents it names", async () => {
    renderPicker({ agentId: "reviewer" });
    const menu = await open();
    expect(
      within(menu)
        .getAllByRole("menuitemcheckbox")
        .map((i) => i.textContent),
    ).toEqual(["Web searchSearch the web.", "Team docs"]);
  });

  it("checks what is chosen, and a choice is the whole set, sorted", async () => {
    const { onChange } = renderPicker({ chosen: ["websearch"] });
    const menu = await open();
    const checked = within(menu).getByRole("menuitemcheckbox", { name: /^Web search/ });
    expect(checked.getAttribute("aria-checked")).toBe("true");
    fireEvent.click(within(menu).getByRole("menuitemcheckbox", { name: /^GitHub/ }));
    expect(onChange).toHaveBeenLastCalledWith(["github", "websearch"]);
    fireEvent.click(checked);
    expect(onChange).toHaveBeenLastCalledWith([]);
    // a choice does not close the menu
    expect(screen.queryByRole("menu")).not.toBeNull();
  });

  it("shows what is chosen as chips with a button each that takes it off, named by the server", () => {
    const { onChange } = renderPicker({ chosen: ["docs", "websearch"] });
    const list = screen.getByRole("list", { name: "Attached tools" });
    expect(
      within(list)
        .getAllByRole("listitem")
        .map((i) => i.textContent),
    ).toEqual(["Team docs", "Web search"]);
    fireEvent.click(screen.getByRole("button", { name: "Remove Web search" }));
    expect(onChange).toHaveBeenLastCalledWith(["docs"]);
  });

  it("keeps the chip of a server that is no longer offered, by its id, so that it can be taken off", () => {
    const { onChange } = renderPicker({ chosen: ["retired"] });
    expect(screen.getByRole("listitem").textContent).toBe("retired");
    fireEvent.click(screen.getByRole("button", { name: "Remove retired" }));
    expect(onChange).toHaveBeenLastCalledWith([]);
  });

  it("draws an icon that is a data: URI and the generic icon for every other, and never an image of a URL", async () => {
    renderPicker({ chosen: ["docs", "websearch"] });
    const menu = await open();
    const images = [...document.querySelectorAll("img")];
    // the menu's item and the chip of Web search; the docs icon is a URL: no image anywhere
    expect(images.map((i) => i.getAttribute("src"))).toEqual([SVG, SVG]);
    expect(within(menu).getAllByRole("menuitemcheckbox")).toHaveLength(3);
    expect(document.querySelectorAll('svg[data-slot="server-icon"]').length).toBeGreaterThanOrEqual(
      3,
    );
  });

  it("reads the list and the agent's card again each time it opens", async () => {
    const v = view();
    const capabilities = caps();
    renderPicker({ view: v, capabilities });
    expect(v.reload).not.toHaveBeenCalled();
    await open();
    expect(v.reload).toHaveBeenCalledTimes(1);
    expect(capabilities.refresh).toHaveBeenCalledTimes(1);
  });

  it("waits while a change is on its way", async () => {
    renderPicker({ busy: true, chosen: ["websearch"] });
    const menu = await open();
    for (const i of within(menu).getAllByRole("menuitemcheckbox")) {
      expect(i.hasAttribute("data-disabled")).toBe(true);
    }
    expect(
      (screen.getByRole("button", { name: "Remove Web search" }) as HTMLButtonElement).disabled,
    ).toBe(true);
  });

  it("draws nothing when there is nothing to attach for the agent and nothing attached", () => {
    const { container } = renderPicker({
      agentId: "reviewer",
      view: view({ servers: [servers[1] as ApiToolServer] }),
    });
    expect(container.textContent).toBe("");
    cleanup();
    expect(renderPicker({ view: view({ servers: [] }) }).container.textContent).toBe("");
  });

  it("draws nothing when the list is not for this person", () => {
    expect(
      renderPicker({ view: view({ unavailable: true, servers: [] }) }).container.textContent,
    ).toBe("");
    cleanup();
    // not even for a chip: no `thread.write`, no picker
    expect(
      renderPicker({ view: view({ unavailable: true, servers: [] }), chosen: ["websearch"] })
        .container.textContent,
    ).toBe("");
  });

  it("says a list that could not be read, in a menu that still has what it had", async () => {
    renderPicker({ view: view({ error: "network down" }) });
    const menu = await open();
    expect(within(menu).getByText("Could not refresh the tools: network down")).toBeTruthy();
    expect(within(menu).getAllByRole("menuitemcheckbox")).toHaveLength(3);
  });

  it("says in the menu that an agent that does not list thread-tools/v1 does not use what is attached", async () => {
    renderPicker({ capabilities: caps(false) });
    const menu = await open();
    expect(within(menu).getByText("This agent does not use attached tools.")).toBeTruthy();
  });

  it("stops at sixteen: the others are disabled, the chosen ones can still be taken off", async () => {
    const many = Array.from({ length: 17 }, (_, i) => ({
      id: `s${String(i).padStart(2, "0")}`,
      name: `S${i}`,
    }));
    const chosen = many.slice(0, 16).map((s) => s.id);
    renderPicker({ view: view({ servers: many }), chosen });
    const menu = await open();
    const items = within(menu).getAllByRole("menuitemcheckbox");
    expect(items.filter((i) => i.hasAttribute("data-disabled"))).toHaveLength(1);
    expect(items.at(-1)?.hasAttribute("data-disabled")).toBe(true);
  });
});

describe("ToolsWarning", () => {
  const text = () => document.querySelector('[data-slot="tools-warning"]')?.textContent ?? null;

  it("says before sending that an agent that does not list thread-tools/v1 will not be sent the tools", () => {
    render(<ToolsWarning chosen={["websearch"]} capabilities={caps(false)} agentName="Reviewer" />);
    expect(text()).toBe("Reviewer cannot use attached tools, so they will not be sent to it.");
    expect(screen.getByRole("status")).toBeTruthy();
  });

  it("says nothing for an agent that lists it, with nothing attached, or before the card is read", () => {
    const check = (el: React.ReactElement) => {
      const { container } = render(el);
      const empty = container.textContent === "";
      cleanup();
      return empty;
    };
    expect(
      check(<ToolsWarning chosen={["websearch"]} capabilities={caps(true)} agentName="Coder" />),
    ).toBe(true);
    expect(
      check(<ToolsWarning chosen={[]} capabilities={caps(false)} agentName="Reviewer" />),
    ).toBe(true);
    expect(
      check(
        <ToolsWarning chosen={["websearch"]} capabilities={caps("unknown")} agentName="Coder" />,
      ),
    ).toBe(true);
  });

  it("does not claim a card it could not read lists the extension: it says it could not check", () => {
    render(
      <ToolsWarning chosen={["websearch"]} capabilities={caps("unreadable")} agentName="Coder" />,
    );
    expect(text()).toBe("Could not check whether Coder can use attached tools.");
  });
});
