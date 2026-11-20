// @vitest-environment jsdom
import { cleanup, configure, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { type ComponentProps, useSyncExternalStore } from "react";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { LiveStream, sse } from "@/features/chat/lib/agui/testing";
import { mountRuntime } from "@/features/chat/lib/agui/testing-runtime";
import type { ThreadAgent } from "@/features/chat/lib/agui/thread-agent";
import type { AgentCommand } from "@/features/commands/lib/commands";
import type { ApiPromptSummary } from "@/lib/api/types";
import { Composer } from "./composer";

/*
 * Which `/` menu the message box opens (ADR 0079, docs/api/commands-v1.md section 6): the agent's
 * own commands when the typed query starts the name of one it serves, and the person's prompt
 * library exactly as before otherwise. The two lists never open together. The commands' own keys and
 * rows are `features/commands/components/command-menu.dom.test.tsx`.
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

const summary = (over: Partial<ApiPromptSummary> = {}): ApiPromptSummary => ({
  id: "p-aaaaaaaaaaaa",
  name: "review",
  title: "Review a change",
  description: "Reviews a change for one concern",
  variableCount: 0,
  own: true,
  revision: "2",
  updatedAt: "2026-11-15T10:00:00Z",
  ...over,
});

/** The person's library holds a prompt called `review` too: the same word the agent serves. */
const prompts = [summary(), summary({ id: "p-bbbbbbbbbbbb", name: "notes", title: "Write notes" })];

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
const commands = () => screen.queryByRole("listbox", { name: "Agent commands" });
const promptsMenu = () => screen.queryByRole("listbox", { name: "Your prompts" });

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
  client.GET.mockImplementation(async (path: string) => {
    if (path === "/api/config") {
      return { data: { ui: {}, prompts: PROMPTS_CONFIG }, response: { status: 200, ok: true } };
    }
    return {
      data: { limit: 100, held: 1, items: prompts },
      response: { status: 200, ok: true },
    };
  });
});
afterEach(cleanup);

describe("the two / menus of the message box", () => {
  it("opens the agent's commands for a query that starts one of their names, and not the prompts", async () => {
    mount({ commands: COMMANDS });
    type("/rev");
    const menu = await screen.findByRole("listbox", { name: "Agent commands" });
    expect(menu.textContent).toContain("/review");
    // the menu says which one it is
    expect(screen.getByText("Agent commands")).toBeTruthy();
    expect(promptsMenu()).toBeNull();
    // the person's library was not even read: the commands took the query before it did
    expect(client.GET.mock.calls.some(([path]) => path === "/api/prompts")).toBe(false);
  });

  it("opens the agent's commands for a bare /, and the prompts only for an agent that serves none", async () => {
    const first = mount({ commands: COMMANDS });
    type("/");
    const menu = await screen.findByRole("listbox", { name: "Agent commands" });
    expect(within(menu).getAllByRole("option")).toHaveLength(1);
    expect(promptsMenu()).toBeNull();
    first.view.unmount();
    cleanup();

    // the same slash, for an agent whose card lists no command: the prompts, as before
    mount();
    type("/");
    const list = await screen.findByRole("listbox", { name: "Your prompts" });
    await waitFor(() => expect(list.textContent).toContain("/review"));
    expect(screen.getByText("Your prompts")).toBeTruthy();
    expect(commands()).toBeNull();
  });

  it("opens the prompts for a query that starts no command, as before", async () => {
    mount({ commands: COMMANDS });
    type("/note");
    const menu = await screen.findByRole("listbox", { name: "Your prompts" });
    await waitFor(() => expect(menu.textContent).toContain("/notes"));
    expect(screen.getByText("Your prompts")).toBeTruthy();
    expect(commands()).toBeNull();
  });

  it("keeps the query after Escape closed the commands, and does not hand it to the prompts", async () => {
    mount({ commands: COMMANDS });
    type("/rev");
    await screen.findByRole("listbox", { name: "Agent commands" });
    key("Escape");
    expect(commands()).toBeNull();
    // the prompts list a `/review` prompt would offer never opens for the same word
    await waitFor(() => expect(promptsMenu()).toBeNull());
    expect(box().value).toBe("/rev");
  });

  it("behaves exactly as before for an agent that serves no command", async () => {
    mount();
    type("/rev");
    const menu = await screen.findByRole("listbox", { name: "Your prompts" });
    await waitFor(() => expect(menu.textContent).toContain("/review"));
    expect(commands()).toBeNull();
  });
});
