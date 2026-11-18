// @vitest-environment jsdom
import { cleanup, configure, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { type ComponentProps, useSyncExternalStore } from "react";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { Composer } from "@/features/chat/components/composer";
import { LiveStream, sse } from "@/features/chat/lib/agui/testing";
import { mountRuntime } from "@/features/chat/lib/agui/testing-runtime";
import type { ThreadAgent } from "@/features/chat/lib/agui/thread-agent";
import type { ApiPrompt, ApiPromptSummary } from "@/lib/api/types";

/*
 * The `/` menu in the message box, against a mocked `/api/prompts`: typing `/name` lists the person's
 * prompts, Enter picks one, the form draws its variables, and `renderPrompt`'s text goes into the
 * box for editing. Nothing is sent; Escape closes the list; without a library nothing opens.
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

/** The config route, and the two prompt routes; `prompts` absent means the deployment has no library. */
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
const list = () => screen.queryByRole("listbox", { name: "Prompts" });
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

describe("the / menu", () => {
  it("lists the prompts when a message is one /name, and Escape closes it", async () => {
    mount();
    type("/rev");
    const menu = await screen.findByRole("listbox", { name: "Prompts" });
    expect(within(menu).getByText("/review")).toBeTruthy();
    expect(within(menu).getByText("Review a change")).toBeTruthy();
    expect(box().getAttribute("aria-expanded")).toBe("true");
    expect(box().getAttribute("aria-controls")).toBe(menu.id);
    const option = within(menu).getByRole("option", { name: /Review a change/ });
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
    const menu = await screen.findByRole("listbox", { name: "Prompts" });
    await waitFor(() => expect(within(menu).getAllByRole("option")).toHaveLength(3));
  });

  it("picks a prompt with Enter, fills its questions and puts the text in the box without sending", async () => {
    const { calls } = mount();
    type("/rev");
    await screen.findByRole("listbox", { name: "Prompts" });
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
    await screen.findByRole("listbox", { name: "Prompts" });
    key("Enter");
    await waitFor(() => expect(box().value).toBe("Write the notes."));
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(client.POST).toHaveBeenCalledWith("/api/prompts/{promptId}/render", {
      params: { path: { promptId: notes.id } },
      body: { revision: "1", answers: [] },
    });
  });

  it("opens the same query again after the command was dismissed and the box cleared", async () => {
    mount();
    type("/notes");
    await screen.findByRole("listbox", { name: "Prompts" });
    key("Escape");
    expect(list()).toBeNull();
    type("");
    type("/notes");
    await screen.findByRole("listbox", { name: "Prompts" });
  });

  it("uses the same prompt again after its text was put in the box", async () => {
    client.POST.mockResolvedValue({
      data: { text: "Write the notes.", revision: "1" },
      response: { status: 200, ok: true },
    });
    mount();
    type("/notes");
    await screen.findByRole("listbox", { name: "Prompts" });
    key("Enter");
    await waitFor(() => expect(box().value).toBe("Write the notes."));

    // the text is the person's; once the box is no longer a command, the same /name opens again
    type("");
    type("/notes");
    await screen.findByRole("listbox", { name: "Prompts" });
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
    await screen.findByRole("listbox", { name: "Prompts" });
    key("Enter");
    await waitFor(() => expect(screen.getByRole("alert").textContent).toContain("32 KiB"));
    expect(box().value).toBe("/notes");
    expect(screen.queryByRole("dialog")).toBeNull();
    // the command is back and the list is open again for it: a second pick is a click away
    await waitFor(() => expect(screen.getByRole("listbox", { name: "Prompts" })).toBeTruthy());
  });

  it("marks a prompt shared with the person, and names no author", async () => {
    mount();
    type("/release");
    const menu = await screen.findByRole("listbox", { name: "Prompts" });
    const option = within(menu).getByRole("option", { name: /Release notes/ });
    expect(within(option).getByText("Shared")).toBeTruthy();
    expect(within(option).queryByText(/@|owner|author/i)).toBeNull();
  });

  it("sends a message that is a slash command when nothing matches", async () => {
    const { calls } = mount();
    type("/zzz");
    const menu = await screen.findByRole("listbox", { name: "Prompts" });
    await waitFor(() => expect(screen.getByText(/No prompt matches/)).toBeTruthy());
    // ARIA's listbox holds options and nothing else: the line is its sibling
    expect(menu.querySelectorAll(":scope > :not([role='option'])")).toHaveLength(0);
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
    const menu = await screen.findByRole("listbox", { name: "Prompts" });
    await waitFor(() => expect(screen.getByText(/could not be loaded/)).toBeTruthy());
    expect(within(menu).queryAllByRole("option")).toHaveLength(0);
  });
});
