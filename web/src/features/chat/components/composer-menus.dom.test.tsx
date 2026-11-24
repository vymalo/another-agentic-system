// @vitest-environment jsdom
import { cleanup, configure, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { type ComponentProps, useSyncExternalStore } from "react";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { LiveStream, sse } from "@/features/chat/lib/agui/testing";
import { mountRuntime } from "@/features/chat/lib/agui/testing-runtime";
import type { ThreadAgent } from "@/features/chat/lib/agui/thread-agent";
import type { AgentCommand } from "@/features/commands/lib/commands";
import type { ApiPrompt, ApiPromptSummary } from "@/lib/api/types";
import { Composer } from "./composer";

/*
 * The one `/` menu of the message box (ADR 0067 slice P3): the agent's own commands (ADR 0079,
 * docs/api/commands-v1.md section 6) and the person's prompt library (docs/api/prompts-v1.md) as two
 * labelled groups of one list, commands first. Which rows each group draws, and the keys, are
 * `features/slash/components/slash-menu.dom.test.tsx`; here it is the two halves together: the order,
 * the box walking both, the pick from either, and the words neither claims.
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

const PROMPTS_CONFIG = {
  limit: 100,
  maxVariables: 8,
  maxBodyBytes: 16384,
  maxRenderedBytes: 32768,
};

/** The person's library holds a prompt called `review` too: the same word the agent serves. */
const summary = (over: Partial<ApiPromptSummary> = {}): ApiPromptSummary => ({
  id: "p-aaaaaaaaaaaa",
  name: "review",
  title: "Review a change",
  description: "Reviews a change for one concern",
  variableCount: 1,
  own: true,
  revision: "2",
  updatedAt: "2026-11-15T10:00:00Z",
  ...over,
});

const prompts = [summary(), summary({ id: "p-bbbbbbbbbbbb", name: "notes", title: "Write notes" })];

const review: ApiPrompt = {
  id: "p-aaaaaaaaaaaa",
  name: "review",
  title: "Review a change",
  description: "Reviews a change for one concern",
  body: "Review for {{focus}}.",
  variables: [{ id: "focus", question: "What focus?", type: "text" }],
  own: true,
  revision: "2",
  updatedAt: "2026-11-15T10:00:00Z",
};

const COMMANDS: readonly AgentCommand[] = [
  { name: "review", description: "Reviews a change.", argumentHint: "<repo>" },
];

const accepting = () => {
  const connect = new LiveStream();
  return (call: { method: string }) => {
    if (call.method === "GET") return sse(connect.body);
    const post = new LiveStream();
    post.frames([
      { event: { type: "RUN_STARTED", threadId: "t", runId: "r", protocolVersion: "1.0" } },
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
const key = (k: string) => fireEvent.keyDown(box(), { key: k });
const list = () => screen.queryByRole("listbox", { name: "Commands and prompts" });
const labels = () =>
  screen.getAllByText(/^(Agent commands|Your prompts)$/).map((node) => node.textContent);

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
  client.GET.mockImplementation(
    async (path: string, init?: { params?: { path?: { promptId?: string } } }) => {
      if (path === "/api/config") {
        return { data: { ui: {}, prompts: PROMPTS_CONFIG }, response: { status: 200, ok: true } };
      }
      if (path === "/api/prompts") {
        return {
          data: { limit: 100, held: 1, items: prompts },
          response: { status: 200, ok: true },
        };
      }
      return init?.params?.path?.promptId === review.id
        ? { data: review, response: { status: 200, ok: true } }
        : { error: { title: "Not found", status: 404 }, response: { status: 404, ok: false } };
    },
  );
  client.POST.mockImplementation(async () => ({
    data: { text: "Review for Security.", revision: "2" },
    response: { status: 200, ok: true },
  }));
});
afterEach(cleanup);

describe("the one / menu of the message box", () => {
  it("draws the agent's commands and the person's prompts as two labelled groups, commands first", async () => {
    mount({ commands: COMMANDS });
    type("/");
    const menu = await screen.findByRole("listbox", { name: "Commands and prompts" });
    await waitFor(() => expect(within(menu).getAllByRole("option")).toHaveLength(3));
    // one list, two names, in the order the epic's table has them
    expect(labels()).toEqual(["Agent commands", "Your prompts"]);
    const [command, ...lines] = within(menu).getAllByRole("option");
    expect(command?.textContent).toContain("/review");
    expect(command?.textContent).toContain("<repo>");
    expect(lines.map((row) => row.textContent)).toEqual([
      expect.stringContaining("/notes"),
      expect.stringContaining("/review"),
    ]);
    // each group is a group of the one listbox, named by its own line
    expect(within(menu).getByRole("group", { name: "Agent commands" })).toBeTruthy();
    expect(within(menu).getByRole("group", { name: "Your prompts" })).toBeTruthy();
  });

  it("walks the two groups as one list, and a pick does what that row always did", async () => {
    mount({ commands: COMMANDS });
    type("/rev");
    const menu = await screen.findByRole("listbox", { name: "Commands and prompts" });
    await waitFor(() => expect(within(menu).getAllByRole("option")).toHaveLength(2));
    const [command, prompt] = within(menu).getAllByRole("option");
    // the box names the first row: the agent's command
    expect(command?.getAttribute("aria-selected")).toBe("true");
    expect(box().getAttribute("aria-activedescendant")).toBe(command?.id);
    // one ArrowDown crosses the group's edge to the prompt
    key("ArrowDown");
    expect(command?.getAttribute("aria-selected")).toBe("false");
    expect(prompt?.getAttribute("aria-selected")).toBe("true");
    expect(box().getAttribute("aria-activedescendant")).toBe(prompt?.id);
    key("ArrowUp");
    expect(command?.getAttribute("aria-selected")).toBe("true");

    // the command's pick inserts /name and a space, and sends nothing
    key("Enter");
    await waitFor(() => expect(box().value).toBe("/review "));
    expect(list()).toBeNull();
    expect(screen.queryByRole("dialog")).toBeNull();

    // the prompt's pick opens its form, as before
    type("/rev");
    await screen.findByRole("listbox", { name: "Commands and prompts" });
    key("ArrowDown");
    key("Enter");
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByRole("heading", { name: "Review a change" })).toBeTruthy();
  });

  it("reads the prompts for a word a command claims too, which is the merge", async () => {
    mount({ commands: COMMANDS });
    type("/rev");
    const menu = await screen.findByRole("listbox", { name: "Commands and prompts" });
    await waitFor(() => expect(within(menu).getAllByRole("option")).toHaveLength(2));
    // both halves were offered for the one word, and the library was read for it
    expect(client.GET.mock.calls.some(([path]) => path === "/api/prompts")).toBe(true);
  });

  it("closes the one list on Escape, keeps the query, and hands it to nobody else", async () => {
    mount({ commands: COMMANDS });
    type("/rev");
    await waitFor(() => expect(list()).not.toBeNull());
    key("Escape");
    expect(list()).toBeNull();
    expect(screen.queryByRole("group", { name: "Your prompts" })).toBeNull();
    expect(box().value).toBe("/rev");
  });

  it("draws the prompts group alone for an agent that serves no command, as before", async () => {
    mount();
    type("/");
    const menu = await screen.findByRole("listbox", { name: "Commands and prompts" });
    await waitFor(() => expect(within(menu).getAllByRole("option")).toHaveLength(2));
    expect(labels()).toEqual(["Your prompts"]);
    expect(screen.queryByRole("group", { name: "Agent commands" })).toBeNull();
  });

  it("draws that group alone for a query no command starts", async () => {
    mount({ commands: COMMANDS });
    type("/note");
    const menu = await screen.findByRole("listbox", { name: "Commands and prompts" });
    await waitFor(() => expect(menu.textContent).toContain("/notes"));
    expect(labels()).toEqual(["Your prompts"]);
    expect(screen.queryByRole("group", { name: "Agent commands" })).toBeNull();
  });
});
