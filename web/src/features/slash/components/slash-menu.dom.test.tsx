// @vitest-environment jsdom
import { cleanup, configure, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { type ComponentProps, useSyncExternalStore } from "react";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { Composer } from "@/features/chat/components/composer";
import { LiveStream, sse } from "@/features/chat/lib/agui/testing";
import { mountRuntime } from "@/features/chat/lib/agui/testing-runtime";
import type { ThreadAgent } from "@/features/chat/lib/agui/thread-agent";
import type { AgentCommand } from "@/features/commands/lib/commands";
import type { ApiPrompt, ApiPromptSummary } from "@/lib/api/types";

/*
 * The one `/` menu of the message box (ADR 0067 slice P3): the agent's own commands (ADR 0079,
 * docs/api/commands-v1.md section 6) and the person's prompts (docs/api/prompts-v1.md) as two
 * labelled groups of one listbox. The commands' rows and keys, the prompts' rows and pick, the fill
 * form — and one list that walks both. Which groups a query opens is `composer-menus.dom.test.tsx`.
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

const COMMANDS: readonly AgentCommand[] = [
  { name: "explain", description: "Explains a file." },
  { name: "review", description: "Reviews a change.", argumentHint: "<repo>" },
];

const summary = (over: Partial<ApiPromptSummary> = {}): ApiPromptSummary => ({
  id: "p-aaaaaaaaaaaa",
  name: "review",
  title: "Review a change",
  description: "",
  variableCount: 1,
  own: true,
  revision: "2",
  updatedAt: "2026-11-15T10:00:00Z",
  ...over,
});

const review: ApiPrompt = {
  id: "p-aaaaaaaaaaaa",
  name: "review",
  title: "Review a change",
  description: "Reviews a change",
  body: "Review for {{focus}}.",
  variables: [{ id: "focus", question: "What focus?", type: "text" }],
  own: true,
  revision: "2",
  updatedAt: "2026-11-15T10:00:00Z",
};

const notes: ApiPrompt = {
  id: "p-bbbbbbbbbbbb",
  name: "notes",
  title: "Write notes",
  description: "",
  body: "Write the notes.",
  variables: [],
  own: true,
  revision: "1",
  updatedAt: "2026-11-15T10:00:00Z",
};

const shared = summary({
  id: "p-cccccccccccc",
  name: "release-notes",
  title: "Release notes",
  description: "Turns merged pull requests into release notes.",
  own: false,
});

const summaryOf = (p: ApiPrompt): ApiPromptSummary => ({
  id: p.id,
  name: p.name,
  title: p.title,
  description: p.description,
  variableCount: p.variables.length,
  own: p.own,
  revision: p.revision,
  updatedAt: p.updatedAt,
});

/** The config route, and the two prompt routes; `config` null means the deployment has no library. */
function answerRoutes({
  config = PROMPTS_CONFIG as object | null,
  list = [summary(), summaryOf(notes), shared],
  full = { [review.id]: review, [notes.id]: notes } as Record<string, ApiPrompt>,
}: {
  config?: object | null;
  list?: ApiPromptSummary[];
  full?: Record<string, ApiPrompt>;
} = {}) {
  client.GET.mockImplementation(
    async (path: string, init?: { params?: { path?: { promptId?: string } } }) => {
      if (path === "/api/config") {
        return {
          data: { ui: {}, ...(config ? { prompts: config } : {}) },
          response: { status: 200, ok: true },
        };
      }
      if (path === "/api/prompts") {
        return {
          data: { limit: 100, held: 1, items: list },
          response: { status: 200, ok: true },
        };
      }
      const id = init?.params?.path?.promptId ?? "";
      const prompt = full[id];
      return prompt
        ? { data: prompt, response: { status: 200, ok: true } }
        : { error: { title: "Not found", status: 404 }, response: { status: 404, ok: false } };
    },
  );
}

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
const list = () => screen.queryByRole("listbox", { name: "Commands and prompts" });
const groupByName = (name: "Agent commands" | "Your prompts") =>
  screen.getByRole("group", { name });
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
  answerRoutes();
  client.POST.mockImplementation(async () => ({
    data: { text: "Review for Security.", revision: "2" },
    response: { status: 200, ok: true },
  }));
});
afterEach(cleanup);

describe("the agent's commands group", () => {
  // a deployment without a prompt library: the commands are the only group that can be drawn
  beforeEach(() => answerRoutes({ config: null }));

  it("lists the agent's commands with their hints and descriptions, the box naming the active one", async () => {
    mount({ commands: COMMANDS });
    type("/");
    const menu = await screen.findByRole("listbox", { name: "Commands and prompts" });
    // the group says which list it is, and the group is what its name names
    const commands = groupByName("Agent commands");
    expect(commands.textContent).toContain("Agent commands");
    expect(screen.queryByRole("group", { name: "Your prompts" })).toBeNull();
    expect(within(commands).getAllByRole("option")).toHaveLength(2);
    const [explain, review_] = within(commands).getAllByRole("option");
    expect(explain?.textContent).toContain("/explain");
    expect(explain?.textContent).toContain("Explains a file.");
    expect(review_?.textContent).toContain("/review");
    expect(review_?.textContent).toContain("<repo>");
    expect(review_?.textContent).toContain("Reviews a change.");
    // the focus stays in the box, which is the combobox of the list
    expect(box().getAttribute("aria-expanded")).toBe("true");
    expect(box().getAttribute("aria-controls")).toBe(menu.id);
    expect(explain?.getAttribute("aria-selected")).toBe("true");
    expect(box().getAttribute("aria-activedescendant")).toBe(explain?.id);
    // the arrows move it, and the box names the row
    key("ArrowDown");
    expect(review_?.getAttribute("aria-selected")).toBe("true");
    expect(box().getAttribute("aria-activedescendant")).toBe(review_?.id);
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
    await screen.findByRole("listbox", { name: "Commands and prompts" });
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
    await screen.findByRole("listbox", { name: "Commands and prompts" });
    key("Tab");
    await waitFor(() => expect(box().value).toBe("/explain"));
    expect(box().selectionStart).toBe(8);
    // the text is still one /word: the list closes because the command was picked, not because of the text
    expect(list()).toBeNull();
  });

  it("closes on Escape and keeps the text, which is the message if it is sent", async () => {
    const { calls } = mount({ commands: COMMANDS });
    type("/rev");
    await screen.findByRole("listbox", { name: "Commands and prompts" });
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
    const menu = await screen.findByRole("listbox", { name: "Commands and prompts" });
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

describe("the person's prompts group", () => {
  it("lists the prompts when a message is one /name, and Escape closes it", async () => {
    mount();
    type("/rev");
    const menu = await screen.findByRole("listbox", { name: "Commands and prompts" });
    const prompts = groupByName("Your prompts");
    expect(within(prompts).getByText("/review")).toBeTruthy();
    expect(within(prompts).getByText("Review a change")).toBeTruthy();
    expect(box().getAttribute("aria-expanded")).toBe("true");
    expect(box().getAttribute("aria-controls")).toBe(menu.id);
    const option = within(prompts).getByRole("option", { name: /Review a change/ });
    expect(option.getAttribute("aria-selected")).toBe("true");
    expect(box().getAttribute("aria-activedescendant")).toBe(option.id);

    key("Escape");
    expect(list()).toBeNull();
    expect(box().value).toBe("/rev");
    expect(box().getAttribute("aria-expanded")).toBeNull();
  });

  it("opens on a slash alone and lists every prompt", async () => {
    mount();
    type("/");
    const menu = await screen.findByRole("listbox", { name: "Commands and prompts" });
    await waitFor(() => expect(within(menu).getAllByRole("option")).toHaveLength(3));
  });

  it("picks a prompt with Enter, fills its questions and puts the text in the box without sending", async () => {
    const { calls } = mount();
    type("/rev");
    await screen.findByRole("listbox", { name: "Commands and prompts" });
    key("Enter");

    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByRole("heading", { name: "Review a change" })).toBeTruthy();
    expect(box().value).toBe("");
    fireEvent.change(within(dialog).getByRole("textbox", { name: "What focus?" }), {
      target: { value: "Security" },
    });
    fireEvent.click(within(dialog).getByRole("button", { name: "Use prompt" }));

    await waitFor(() => expect(box().value).toBe("Review for Security."));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(client.POST).toHaveBeenCalledWith("/api/prompts/{promptId}/render", {
      params: { path: { promptId: review.id } },
      body: { revision: "2", answers: [{ id: "focus", values: [], text: "Security" }] },
    });
    // nothing was sent: the person edits and sends it
    expect(posts(calls)).toHaveLength(0);
  });

  it("fills a prompt with no questions with answers: [] and no form", async () => {
    client.POST.mockResolvedValueOnce({
      data: { text: "Write the notes.", revision: "1" },
      response: { status: 200, ok: true },
    });
    mount();
    type("/notes");
    await screen.findByRole("listbox", { name: "Commands and prompts" });
    key("Enter");
    await waitFor(() => expect(box().value).toBe("Write the notes."));
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(client.POST).toHaveBeenCalledWith("/api/prompts/{promptId}/render", {
      params: { path: { promptId: notes.id } },
      body: { revision: "1", answers: [] },
    });
  });

  it("opens the same query again after the list was dismissed and the box cleared", async () => {
    mount();
    type("/notes");
    await screen.findByRole("listbox", { name: "Commands and prompts" });
    key("Escape");
    expect(list()).toBeNull();
    type("");
    type("/notes");
    await screen.findByRole("listbox", { name: "Commands and prompts" });
  });

  it("uses the same prompt again after its text was put in the box", async () => {
    client.POST.mockResolvedValue({
      data: { text: "Write the notes.", revision: "1" },
      response: { status: 200, ok: true },
    });
    mount();
    type("/notes");
    await screen.findByRole("listbox", { name: "Commands and prompts" });
    key("Enter");
    await waitFor(() => expect(box().value).toBe("Write the notes."));

    // the text is the person's; once the box is no longer a command, the same /name opens again
    type("");
    type("/notes");
    await screen.findByRole("listbox", { name: "Commands and prompts" });
    key("Enter");
    await waitFor(() => expect(box().value).toBe("Write the notes."));
  });

  it("offers nothing when the configuration has no library, and asks for no prompts", async () => {
    answerRoutes({ config: null });
    mount();
    // the configuration was read: the absence of the menu below is not the absence of an answer
    await waitFor(() =>
      expect(client.GET.mock.calls.some(([path]) => path === "/api/config")).toBe(true),
    );
    type("/");
    await waitFor(() => expect(box().getAttribute("aria-expanded")).toBeNull());
    expect(list()).toBeNull();
    expect(client.GET.mock.calls.some(([path]) => path === "/api/prompts")).toBe(false);
  });

  it("says a refused fill above the box, and gives the command back", async () => {
    client.POST.mockResolvedValueOnce({
      error: { code: "rendered_too_long", status: 422 },
      response: { status: 422, ok: false },
    });
    mount();
    type("/notes");
    await screen.findByRole("listbox", { name: "Commands and prompts" });
    key("Enter");
    await waitFor(() => expect(screen.getByRole("alert").textContent).toContain("32 KiB"));
    expect(box().value).toBe("/notes");
    expect(screen.queryByRole("dialog")).toBeNull();
    // the command is back and the list is open again for it: a second pick is a click away
    await waitFor(() =>
      expect(screen.getByRole("listbox", { name: "Commands and prompts" })).toBeTruthy(),
    );
  });

  it("marks a prompt shared with the person, and names no author", async () => {
    mount();
    type("/release");
    const menu = await screen.findByRole("listbox", { name: "Commands and prompts" });
    const option = within(menu).getByRole("option", { name: /Release notes/ });
    expect(within(option).getByText("Shared")).toBeTruthy();
    expect(within(option).queryByText(/@|owner|author/i)).toBeNull();
  });

  it("sends a message that is a slash command when nothing matches", async () => {
    const { calls } = mount();
    type("/zzz");
    await waitFor(() => expect(screen.getByText(/No prompt matches/)).toBeTruthy());
    // no row at all: no listbox (an empty one is invalid), just the prompts' block, which the box controls
    expect(list()).toBeNull();
    expect(screen.queryAllByRole("option")).toHaveLength(0);
    expect(screen.getByText("Your prompts")).toBeTruthy();
    expect(box().getAttribute("aria-expanded")).toBe("true");
    key("Enter");
    await waitFor(() => expect(posts(calls)).toHaveLength(1));
    expect(posts(calls)[0]?.messages[0]?.content).toBe("/zzz");
  });

  it("says when the list cannot be read, and offers no row", async () => {
    const routes = client.GET.getMockImplementation() as (path: string, init?: unknown) => unknown;
    client.GET.mockImplementation(async (path: string, init?: unknown) =>
      path === "/api/prompts"
        ? {
            error: { title: "Unavailable", status: 503 },
            response: { status: 503, ok: false },
          }
        : routes(path, init),
    );
    mount();
    type("/rev");
    await waitFor(() => expect(screen.getByText(/could not be loaded/)).toBeTruthy());
    expect(screen.queryAllByRole("option")).toHaveLength(0);
    // the line is a block, not an empty listbox
    expect(list()).toBeNull();
  });
});
