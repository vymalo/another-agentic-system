// @vitest-environment jsdom
import { cleanup, configure, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { type ComponentProps, useSyncExternalStore } from "react";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { Composer } from "@/features/chat/components/composer";
import { LiveStream, sse } from "@/features/chat/lib/agui/testing";
import { mountRuntime } from "@/features/chat/lib/agui/testing-runtime";
import type { ThreadAgent } from "@/features/chat/lib/agui/thread-agent";
import type { AgentCommand } from "../lib/commands";

/*
 * The `/` menu of the agent's own commands in the message box (ADR 0079, docs/api/commands-v1.md
 * section 6): the rows with their hints, the keys the box keeps, the text a pick inserts — and
 * nothing sent. The deployment here offers no prompt library, so the only list that can open is the
 * agent's; the arbitration between the two lists is `composer-menus.dom.test.tsx`.
 */
const client = vi.hoisted(() => ({
  GET: vi.fn(),
  POST: vi.fn(),
  PATCH: vi.fn(),
  DELETE: vi.fn(),
}));
vi.mock("@/lib/api/client", async () => ({
  ...(await vi.importActual<typeof import("@/lib/api/client")>("@/lib/api/client")),
  api: client,
}));

let resetUiConfig: typeof import("@/features/chat/hooks/use-ui-config").resetUiConfig;

const COMMANDS: readonly AgentCommand[] = [
  { name: "explain", description: "Explains a file." },
  { name: "review", description: "Reviews a change.", argumentHint: "<repo>" },
];

const accepting = () => {
  const connect = new LiveStream();
  return (call: { method: string }) => {
    if (call.method === "GET") return sse(connect.body);
    const post = new LiveStream();
    post.frames([
      {
        event: { type: "RUN_STARTED", threadId: "t", runId: "r", protocolVersion: "1.0" },
      },
    ]);
    return sse(post.body);
  };
};

type Props = Partial<ComponentProps<typeof Composer>>;

function mount(props: Props = {}) {
  const mounted = mountRuntime(accepting(), {}, (agent: ThreadAgent) => (
    <Wired agent={agent} {...props} />
  ));
  mounted.agent.start();
  return mounted;
}

function Wired({ agent, ...rest }: Props & { agent: ThreadAgent }) {
  const snapshot = useSyncExternalStore(agent.onChange, agent.getSnapshot, agent.getSnapshot);
  return (
    <Composer
      state="done"
      isNew={false}
      sendError={null}
      onCancel={() => {}}
      sending={{
        send: (text, mode) => agent.sendWhileWorking(text, mode),
        onFailed: () => {},
        agent: "Coder",
        steers: true,
        ready: !snapshot.replaying,
      }}
      {...rest}
    />
  );
}

const box = () => screen.getByLabelText("Message") as HTMLTextAreaElement;
const type = (text: string) => fireEvent.change(box(), { target: { value: text } });
const key = (k: string, init: Record<string, unknown> = {}) =>
  fireEvent.keyDown(box(), { key: k, ...init });
const list = () => screen.queryByRole("listbox", { name: "Agent commands" });
const options = () => screen.getAllByRole("option");
const posts = (calls: { method: string; body?: unknown }[]) =>
  calls
    .filter((c) => c.method === "POST")
    .map((c) => c.body as { messages: { content: string }[] });

beforeAll(async () => {
  configure({ asyncUtilTimeout: 10_000 });
  globalThis.ResizeObserver = class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
  Element.prototype.scrollTo = () => {};
  Element.prototype.scrollIntoView = () => {};
  ({ resetUiConfig } = await import("@/features/chat/hooks/use-ui-config"));
});

beforeEach(() => {
  resetUiConfig();
  client.GET.mockReset();
  client.POST.mockReset();
  client.PATCH.mockReset();
  client.DELETE.mockReset();
  // a deployment without a prompt library: no other `/` menu can open here
  client.GET.mockImplementation(async () => ({
    data: { ui: {} },
    response: { status: 200, ok: true },
  }));
});
afterEach(cleanup);

describe("the / menu of the agent's commands", () => {
  it("lists the agent's commands with their hints and descriptions, the box naming the active one", async () => {
    mount({ commands: COMMANDS });
    type("/");
    const menu = await screen.findByRole("listbox", { name: "Agent commands" });
    // the list says which one it is, beside the listbox and not in it
    expect(screen.getByText("Agent commands")).toBeTruthy();
    expect(within(menu).queryByText("Agent commands")).toBeNull();
    expect(within(menu).getAllByRole("option")).toHaveLength(2);
    const [explain, review] = within(menu).getAllByRole("option");
    expect(explain?.textContent).toContain("/explain");
    expect(explain?.textContent).toContain("Explains a file.");
    expect(review?.textContent).toContain("/review");
    expect(review?.textContent).toContain("<repo>");
    expect(review?.textContent).toContain("Reviews a change.");
    // the focus stays in the box, which is the combobox of the list
    expect(box().getAttribute("aria-expanded")).toBe("true");
    expect(box().getAttribute("aria-controls")).toBe(menu.id);
    expect(explain?.getAttribute("aria-selected")).toBe("true");
    expect(box().getAttribute("aria-activedescendant")).toBe(explain?.id);
    // the arrows move it, and the box names the row
    key("ArrowDown");
    expect(review?.getAttribute("aria-selected")).toBe("true");
    expect(box().getAttribute("aria-activedescendant")).toBe(review?.id);
    key("ArrowUp");
    expect(explain?.getAttribute("aria-selected")).toBe("true");
    // a narrowed query offers only what starts with it
    type("/re");
    await waitFor(() => expect(options()).toHaveLength(1));
    expect(options()[0]?.textContent).toContain("/review");
  });

  it("inserts /name and a space with Enter, the caret at the end, and sends nothing", async () => {
    const { calls } = mount({ commands: COMMANDS });
    type("/rev");
    await screen.findByRole("listbox", { name: "Agent commands" });
    key("Enter");
    await waitFor(() => expect(box().value).toBe("/review "));
    // the caret is after the space: the arguments are typed there
    expect(box().selectionStart).toBe(8);
    // the text stopped being one /word, so the list closed by itself
    expect(list()).toBeNull();
    expect(posts(calls)).toHaveLength(0);
  });

  it("inserts /name alone for a command that takes no arguments, and closes the list on the pick", async () => {
    mount({ commands: COMMANDS });
    type("/explain");
    await screen.findByRole("listbox", { name: "Agent commands" });
    key("Tab");
    await waitFor(() => expect(box().value).toBe("/explain"));
    expect(box().selectionStart).toBe(8);
    // the text is still one /word: the list closes because the command was picked, not because of the text
    expect(list()).toBeNull();
  });

  it("closes on Escape and keeps the text, which is the message if it is sent", async () => {
    const { calls } = mount({ commands: COMMANDS });
    type("/rev");
    await screen.findByRole("listbox", { name: "Agent commands" });
    key("Escape");
    expect(list()).toBeNull();
    expect(box().value).toBe("/rev");
    key("Enter");
    await waitFor(() => expect(posts(calls)).toHaveLength(1));
    expect(posts(calls)[0]?.messages[0]?.content).toBe("/rev");
  });

  it("picks with a click, which does not take the focus from the box", async () => {
    mount({ commands: COMMANDS });
    type("/re");
    const menu = await screen.findByRole("listbox", { name: "Agent commands" });
    const option = within(menu).getByRole("option", { name: /Reviews a change/ });
    fireEvent.mouseDown(option);
    fireEvent.click(option);
    await waitFor(() => expect(box().value).toBe("/review "));
  });

  it("draws no list when no command matches, and none for an agent that serves none", async () => {
    mount({ commands: COMMANDS });
    type("/zzz");
    await waitFor(() => expect(list()).toBeNull());
    cleanup();
    mount({ commands: [] });
    type("/rev");
    await waitFor(() => expect(list()).toBeNull());
    expect(box().getAttribute("aria-expanded")).toBeNull();
  });
});
