// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { AgentsView } from "@/features/agents/hooks/use-agents";
import type { Selection } from "@/features/chat/hooks/use-chat-runtime";
import type { ApiAgent } from "@/lib/api/types";
import { AgentMenu, type AgentMenuProps } from "./agent-menu";

afterEach(cleanup);

// Radix measures its popper content; jsdom has no layout
globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
};

const coder: ApiAgent = {
  id: "coder",
  name: "Coder",
  description: "Implements a change and opens a pull request.",
  releases: {
    defaultChannel: "production",
    channels: { production: "coder-r47", staging: "coder-r51" },
    revisions: ["coder-r53", "coder-r51", "coder-r47"],
  },
};
const reviewer: ApiAgent = {
  id: "reviewer",
  name: "Reviewer",
  description: "Reviews a pull request and reports findings.",
};
const plain: ApiAgent = { id: "plain", name: "Plain" };

const view = (over: Partial<AgentsView> = {}): AgentsView => ({
  agents: [coder, reviewer, plain],
  loading: false,
  error: null,
  retry: vi.fn(),
  ...over,
});

type NewProps = Extract<AgentMenuProps, { mode: "new" }>;

function renderNew(over: Partial<NewProps> = {}) {
  const onChange = vi.fn<(next: Selection) => void>();
  const agents = over.agents ?? view();
  const utils = render(
    <AgentMenu
      mode="new"
      agents={agents}
      value={{ agentId: "coder", release: "production" }}
      onChange={onChange}
      {...over}
    />,
  );
  return { ...utils, onChange, agents };
}

const trigger = () => screen.getByRole("button", { name: /^Agent:/ });
/** Opens the menu the way a keyboard does (Enter on the button). */
function open() {
  trigger().focus();
  fireEvent.keyDown(trigger(), { key: "Enter" });
  return screen.findByRole("menu");
}
const item = (menu: HTMLElement, name: RegExp) => within(menu).getByRole("menuitemradio", { name });

describe("AgentMenu on a new chat", () => {
  it("is a menu button named for the agent, with its release when it has one", () => {
    renderNew();
    const button = trigger();
    expect(button.getAttribute("aria-haspopup")).toBe("menu");
    expect(button.getAttribute("aria-expanded")).toBe("false");
    // the name a screen reader hears starts with what the control is, then what the person sees
    expect(button.textContent).toBe("Agent: Coder · production");
  });

  it("lists the agents as radio items: the chosen one checked, each with its description", async () => {
    renderNew();
    const menu = await open();
    expect(group("Agents", menu)).toBeTruthy();
    const coderItem = item(menu, /^Coder/);
    expect(coderItem.getAttribute("aria-checked")).toBe("true");
    expect(coderItem.textContent).toContain("Implements a change and opens a pull request.");
    expect(item(menu, /^Reviewer/).getAttribute("aria-checked")).toBe("false");
    expect(item(menu, /^Plain/).getAttribute("aria-checked")).toBe("false");
    // an agent without a description is just its name
    expect(item(menu, /^Plain/).textContent).toBe("PPlain");
  });

  it("offers the release in the same menu: channels with their revision, then the revisions", async () => {
    renderNew();
    const menu = await open();
    expect(group("Release", menu)).toBeTruthy();
    expect(item(menu, /^production/).getAttribute("aria-checked")).toBe("true");
    expect(item(menu, /^production/).textContent).toBe("production — coder-r47");
    expect(item(menu, /^staging/).getAttribute("aria-checked")).toBe("false");
    expect(within(menu).getAllByRole("menuitemradio", { name: /^coder-r\d+$/ })).toHaveLength(3);
  });

  it("has no release group for an agent without releases", async () => {
    renderNew({ value: { agentId: "reviewer", release: null } });
    expect(trigger().textContent).toBe("Agent: Reviewer");
    const menu = await open();
    expect(within(menu).queryByRole("group", { name: "Release" })).toBeNull();
  });

  it("defaults the release shown to the agent's default channel", () => {
    renderNew({ value: { agentId: "coder", release: null } });
    expect(trigger().textContent).toBe("Agent: Coder · production");
  });

  it("choosing an agent reports it with no release and closes the menu", async () => {
    const { onChange } = renderNew();
    const menu = await open();
    fireEvent.click(item(menu, /^Reviewer/));
    expect(onChange).toHaveBeenCalledWith({ agentId: "reviewer", release: null });
    await waitFor(() => expect(screen.queryByRole("menu")).toBeNull());
  });

  it("choosing a channel or a revision reports it for the same agent", async () => {
    const { onChange } = renderNew();
    let menu = await open();
    fireEvent.click(item(menu, /^staging/));
    expect(onChange).toHaveBeenLastCalledWith({ agentId: "coder", release: "staging" });
    await waitFor(() => expect(screen.queryByRole("menu")).toBeNull());
    menu = await open();
    fireEvent.click(item(menu, /^coder-r53$/));
    expect(onChange).toHaveBeenLastCalledWith({ agentId: "coder", release: "coder-r53" });
  });

  it("reads the agents again each time it opens (releases are read live)", async () => {
    const agents = view();
    renderNew({ agents });
    expect(agents.retry).not.toHaveBeenCalled();
    await open();
    expect(agents.retry).toHaveBeenCalledTimes(1);
    fireEvent.keyDown(screen.getByRole("menu"), { key: "Escape" });
    await waitFor(() => expect(screen.queryByRole("menu")).toBeNull());
    await open();
    expect(agents.retry).toHaveBeenCalledTimes(2);
  });

  it("keeps the list and says so when a refresh fails, with Retry", async () => {
    const agents = view({ error: "the agent directory is down" });
    renderNew({ agents });
    const menu = await open();
    expect(item(menu, /^Coder/)).toBeTruthy();
    expect(within(menu).getByRole("status").textContent).toBe(
      "Could not refresh the agents: the agent directory is down",
    );
    fireEvent.click(within(menu).getByRole("menuitem", { name: "Retry" }));
    expect(agents.retry).toHaveBeenCalled();
  });

  it("has a slot for a notice under the lists (the registry's, plan 05)", async () => {
    renderNew({ notice: <p>The agent registry is unreachable.</p> });
    const menu = await open();
    expect(within(menu).getByText("The agent registry is unreachable.")).toBeTruthy();
  });

  it("is a skeleton while the first list loads, and says what is wrong when it did not", async () => {
    const loading = renderNew({
      agents: view({ agents: [], loading: true }),
      value: { agentId: null, release: null },
    });
    expect(screen.getByRole("status").textContent).toBe("Loading agents…");
    expect(screen.queryByRole("button")).toBeNull();
    loading.unmount();

    const failed = view({ agents: [], error: "boom" });
    renderNew({ agents: failed, value: { agentId: null, release: null } });
    expect(trigger().textContent).toBe("Agent: Agents unavailable");
    const menu = await open();
    fireEvent.click(within(menu).getByRole("menuitem", { name: "Retry loading the agents" }));
    expect(failed.retry).toHaveBeenCalled();
  });

  it("with no agents configured, says so on a disabled button", () => {
    renderNew({ agents: view({ agents: [] }), value: { agentId: null, release: null } });
    expect((screen.getByRole("button", { name: "No agents" }) as HTMLButtonElement).disabled).toBe(
      true,
    );
  });

  it("falls back to the first agent when the choice is not in the list", () => {
    renderNew({ value: { agentId: "gone", release: null } });
    expect(trigger().textContent).toBe("Agent: Coder · production");
  });
});

describe("AgentMenu from the keyboard", () => {
  it("opens on Enter and on Space, and Escape closes it and gives the focus back", async () => {
    renderNew();
    const button = trigger();
    button.focus();
    fireEvent.keyDown(button, { key: "Enter" });
    expect(await screen.findByRole("menu")).toBeTruthy();
    expect(button.getAttribute("aria-expanded")).toBe("true");
    fireEvent.keyDown(screen.getByRole("menu"), { key: "Escape" });
    await waitFor(() => expect(screen.queryByRole("menu")).toBeNull());
    expect(document.activeElement).toBe(button);

    fireEvent.keyDown(button, { key: " " });
    expect(await screen.findByRole("menu")).toBeTruthy();
  });

  it("moves between the items with the arrows and chooses with Enter", async () => {
    const { onChange } = renderNew();
    trigger().focus();
    fireEvent.keyDown(trigger(), { key: "ArrowDown" });
    const menu = await screen.findByRole("menu");
    // the first item takes the focus; the arrows go down and up the items of both groups
    const focused = () => document.activeElement as Element;
    await waitFor(() => expect(focused()).toBe(item(menu, /^Coder/)));
    // Radix moves the focus a tick after the key
    fireEvent.keyDown(focused(), { key: "ArrowDown" });
    await waitFor(() => expect(focused()).toBe(item(menu, /^Reviewer/)));
    fireEvent.keyDown(focused(), { key: "ArrowDown" });
    await waitFor(() => expect(focused()).toBe(item(menu, /^Plain/)));
    fireEvent.keyDown(focused(), { key: "ArrowUp" });
    await waitFor(() => expect(focused()).toBe(item(menu, /^Reviewer/)));
    fireEvent.keyDown(focused(), { key: "Enter" });
    expect(onChange).toHaveBeenCalledWith({ agentId: "reviewer", release: null });
  });
});

describe("AgentMenu on an existing thread", () => {
  const thread = (over: { release?: string | null; agents?: AgentsView } = {}) =>
    render(
      <AgentMenu
        mode="thread"
        agents={over.agents ?? view()}
        value={{ agentId: "coder", release: over.release ?? null }}
      />,
    );

  it("shows the thread's own agent, checked, and its pinned release", async () => {
    thread({ release: "staging" });
    expect(trigger().textContent).toBe("Agent: Coder · staging");
    const menu = await open();
    const current = item(menu, /^Coder/);
    expect(current.getAttribute("aria-checked")).toBe("true");
    expect(current.textContent).toContain("staging");
    // no release group: a thread keeps its release too
    expect(within(menu).queryByRole("group", { name: "Release" })).toBeNull();
  });

  it("offers the other agents as links that start a new chat with them", async () => {
    thread();
    const menu = await open();
    // they are not radio items of this chat: nothing here changes the thread's agent
    expect(within(menu).queryByRole("menuitemradio", { name: /^Reviewer/ })).toBeNull();
    const group = within(menu).getByRole("group", { name: "Start a new chat with" });
    const links = within(group).getAllByRole("menuitem");
    expect(links.map((l) => l.getAttribute("href"))).toEqual(["/?agent=reviewer", "/?agent=plain"]);
    expect(links[0]?.textContent).toContain("Reviews a pull request");
  });

  it("names the agent by its id until the list is read, and still opens", async () => {
    thread({ agents: view({ agents: [], loading: true }) });
    expect(trigger().textContent).toBe("Agent: coder");
    const menu = await open();
    expect(item(menu, /^coder/).getAttribute("aria-checked")).toBe("true");
  });

  it("is a skeleton until the thread (and so its agent) is known", () => {
    render(<AgentMenu mode="thread" agents={view()} value={{ agentId: null, release: null }} />);
    expect(screen.getByRole("status").textContent).toBe("Loading agent…");
  });
});

/** The radio group named `name` inside the menu. */
function group(name: string, menu: HTMLElement) {
  return within(menu).getByRole("group", { name });
}
