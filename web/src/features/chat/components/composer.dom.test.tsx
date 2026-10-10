// @vitest-environment jsdom
import {
  act,
  cleanup,
  configure,
  fireEvent,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { type ComponentProps, useSyncExternalStore } from "react";
import { afterEach, describe, expect, it } from "vitest";
import { TooltipProvider } from "@/components/ui/tooltip";
import { LiveStream, problem, sse } from "@/features/chat/lib/agui/testing";
import { mountRuntime } from "@/features/chat/lib/agui/testing-runtime";
import type { ThreadAgent } from "@/features/chat/lib/agui/thread-agent";
import { Composer } from "./composer";

configure({ asyncUtilTimeout: 10_000 });
afterEach(cleanup);

// Radix measures its popper content; jsdom has no layout
globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
};

type Body = {
  runId: string;
  messages: { role: string; content: string }[];
  forwardedProps: Record<string, unknown>;
};

/** An orchestrator that accepts every run: `RUN_STARTED` and nothing more. */
const accepting = () => {
  const connect = new LiveStream();
  return (call: { method: string; body?: unknown }) => {
    if (call.method === "GET") return sse(connect.body);
    const post = new LiveStream();
    post.frames([
      {
        event: {
          type: "RUN_STARTED",
          threadId: "t",
          runId: (call.body as Body).runId,
          protocolVersion: "1.0",
        },
      },
    ]);
    return sse(post.body);
  };
};

/** What the composer told the page about a message that was refused. */
const failures: string[] = [];
afterEach(() => {
  failures.length = 0;
});

type Props = Partial<ComponentProps<typeof Composer>> & {
  /** What the thread is doing; the box of a working thread is the one under test. */
  state?: ComponentProps<typeof Composer>["state"];
  /** Whether the transcript is on screen at first: a send waits for it (the replay's guard). `show` says when it is. */
  shown?: boolean;
};

/** Whether the transcript is on screen, which the page knows and the test changes. */
const screenGate = { shown: true, listeners: new Set<() => void>() };
const show = (shown = true) => {
  screenGate.shown = shown;
  for (const l of [...screenGate.listeners]) l();
};
const useShown = () =>
  useSyncExternalStore(
    (l) => {
      screenGate.listeners.add(l);
      return () => void screenGate.listeners.delete(l);
    },
    () => screenGate.shown,
    () => screenGate.shown,
  );

/** The composer on the real runtime and a ThreadAgent, as chat-shell wires it. */
function mount(handler = accepting(), props: Props = {}) {
  const { shown = true, state = "working", ...rest } = props;
  screenGate.shown = shown;
  const mounted = mountRuntime(handler, {}, (agent: ThreadAgent) => (
    <Wired agent={agent} state={state} {...rest} />
  ));
  mounted.agent.start();
  return mounted;
}

function Wired({ agent, state, ...rest }: Props & { agent: ThreadAgent; state: Props["state"] }) {
  const snapshot = useSyncExternalStore(agent.onChange, agent.getSnapshot, agent.getSnapshot);
  const shown = useShown();
  return (
    <TooltipProvider>
      <Composer
        state={state}
        isNew={false}
        sendError={null}
        onCancel={() => {}}
        sending={{
          send: (text, mode) => agent.sendWhileWorking(text, mode),
          onFailed: (message) => void failures.push(message),
          agent: "Coder",
          steers: true,
          ready: shown && !snapshot.replaying,
        }}
        {...rest}
      />
    </TooltipProvider>
  );
}

const box = () => screen.getByLabelText("Message") as HTMLTextAreaElement;
const type = (text: string) => fireEvent.change(box(), { target: { value: text } });
const send = () => screen.getByRole("button", { name: "Send" });
const more = () => screen.getByRole("button", { name: "Delivery options" });
const posts = (calls: { method: string; body?: unknown }[]) =>
  calls.filter((c) => c.method === "POST").map((c) => c.body as Body);

async function openMenu() {
  more().focus();
  fireEvent.keyDown(more(), { key: "Enter" });
  return screen.findByRole("menu");
}

describe("the composer while the agent works", () => {
  it("stays open, keeps Stop, and offers Send with a menu only when there is something to send", () => {
    mount();
    expect(box().disabled).toBe(false);
    expect(screen.getByRole("button", { name: "Stop" })).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Send" })).toBeNull();
    type("you were wrong since line 1");
    expect(send()).toBeTruthy();
    expect(more().getAttribute("aria-haspopup")).toBe("menu");
    // Stop is still there beside them
    expect(screen.getByRole("button", { name: "Stop" })).toBeTruthy();
    // Send says what it does, for a screen reader
    expect(send().getAttribute("aria-describedby")).toBeTruthy();
    expect(
      document.getElementById(send().getAttribute("aria-describedby") ?? "")?.textContent,
    ).toBe("Coder reads it at its next step");
  });

  it("Send posts the message with vymalo.send: steer, and the box is emptied", async () => {
    const { calls } = mount();
    type("you were wrong since line 1");
    fireEvent.click(send());
    await waitFor(() => expect(posts(calls)).toHaveLength(1));
    const [post] = posts(calls);
    expect(post?.messages).toHaveLength(1);
    expect(post?.messages[0]).toMatchObject({
      role: "user",
      content: "you were wrong since line 1",
    });
    expect(post?.forwardedProps["vymalo.send"]).toBe("steer");
    expect(box().value).toBe("");
  });

  it("Stop and send, from the menu, posts vymalo.send: interrupt", async () => {
    const { calls } = mount();
    type("do X instead");
    const menu = await openMenu();
    const items = within(menu).getAllByRole("menuitem");
    expect(items.map((i) => i.querySelector("span > span")?.textContent)).toEqual([
      "Send",
      "Stop and send",
    ]);
    fireEvent.click(within(menu).getByRole("menuitem", { name: /Stop and send/ }));
    await waitFor(() => expect(posts(calls)).toHaveLength(1));
    expect(posts(calls)[0]?.forwardedProps["vymalo.send"]).toBe("interrupt");
    expect(posts(calls)[0]?.messages[0]).toMatchObject({ content: "do X instead" });
    // the menu says what each does, and the keys
    expect(within(menu).getByText("Stops Coder and starts again with your message")).toBeTruthy();
    expect(within(menu).getByText("Coder reads it at its next step")).toBeTruthy();
    expect(within(menu).getByText("Ctrl/⌘ Shift Enter")).toBeTruthy();
  });

  it("the menu's Send is Send", async () => {
    const { calls } = mount();
    type("one more thing");
    const menu = await openMenu();
    fireEvent.click(within(menu).getByRole("menuitem", { name: /^Send/ }));
    await waitFor(() => expect(posts(calls)).toHaveLength(1));
    expect(posts(calls)[0]?.forwardedProps["vymalo.send"]).toBe("steer");
  });

  it("the keys: Enter sends, Ctrl or Cmd with Shift stops and sends, Shift+Enter is a new line", async () => {
    const { calls } = mount();
    type("first");
    fireEvent.keyDown(box(), { key: "Enter", shiftKey: true });
    await new Promise((r) => setTimeout(r, 30));
    expect(posts(calls)).toHaveLength(0);
    expect(box().value).toBe("first");

    fireEvent.keyDown(box(), { key: "Enter" });
    await waitFor(() => expect(posts(calls)).toHaveLength(1));
    expect(posts(calls)[0]?.forwardedProps["vymalo.send"]).toBe("steer");

    type("second");
    fireEvent.keyDown(box(), { key: "Enter", ctrlKey: true, shiftKey: true });
    await waitFor(() => expect(posts(calls)).toHaveLength(2));
    expect(posts(calls)[1]?.forwardedProps["vymalo.send"]).toBe("interrupt");

    type("third");
    fireEvent.keyDown(box(), { key: "Enter", metaKey: true, shiftKey: true });
    await waitFor(() => expect(posts(calls)).toHaveLength(3));
    expect(posts(calls)[2]?.forwardedProps["vymalo.send"]).toBe("interrupt");
  });

  it("an empty box sends nothing, and a message in the middle of an IME composition is not sent", async () => {
    const { calls } = mount();
    fireEvent.keyDown(box(), { key: "Enter" });
    type("日本");
    fireEvent.keyDown(box(), { key: "Enter", isComposing: true });
    await new Promise((r) => setTimeout(r, 30));
    expect(posts(calls)).toHaveLength(0);
    expect(box().value).toBe("日本");
  });

  it("holds a send back until the conversation is on screen: the buttons are disabled and Enter does nothing", async () => {
    const { calls } = mount(accepting(), { shown: false });
    type("too early");
    expect((send() as HTMLButtonElement).disabled).toBe(true);
    expect((more() as HTMLButtonElement).disabled).toBe(true);
    expect(send().getAttribute("title")).toBe("Loading the conversation…");
    fireEvent.keyDown(box(), { key: "Enter" });
    fireEvent.keyDown(box(), { key: "Enter", ctrlKey: true, shiftKey: true });
    await new Promise((r) => setTimeout(r, 30));
    expect(posts(calls)).toHaveLength(0);
    // what was typed is still there
    expect(box().value).toBe("too early");
  });

  it("a refused message comes back into the box, in front of what was written since, and the page is told why", async () => {
    const connect = new LiveStream();
    const { calls } = mount((call) =>
      call.method === "GET"
        ? sse(connect.body)
        : problem(422, "Unprocessable", "the message is too long"),
    );
    type("try this");
    fireEvent.click(send());
    // written while the request was on its way
    type("and this");
    await waitFor(() => expect(failures).toEqual(["the message is too long"]));
    await waitFor(() => expect(box().value).toBe("try this\n\nand this"));
    expect(posts(calls)[0]?.forwardedProps["vymalo.send"]).toBe("steer");
    // nothing was added to the transcript: the message is not drawn until the log says it
    expect(screen.queryByText("try this")).toBeNull();
  });

  it("a request that fails brings the text back, too", async () => {
    const connect = new LiveStream();
    mount((call) => {
      if (call.method === "GET") return sse(connect.body);
      throw new TypeError("Failed to fetch");
    });
    type("try this");
    fireEvent.click(send());
    await waitFor(() => expect(box().value).toBe("try this"));
    expect(failures).toHaveLength(1);
  });
});

describe("the composer of an idle thread", () => {
  for (const state of ["done", "failed", "cancelled"] as const) {
    it(`${state}: Send is the runtime's own, no split menu, and the run carries no vymalo.send`, async () => {
      const { calls } = mount(accepting(), { state });
      type("echo go on");
      expect(screen.queryByRole("button", { name: "Delivery options" })).toBeNull();
      expect(screen.queryByRole("button", { name: "Stop" })).toBeNull();
      fireEvent.click(send());
      await waitFor(() => expect(posts(calls)).toHaveLength(1));
      expect(posts(calls)[0]?.forwardedProps).not.toHaveProperty("vymalo.send");
    });
  }

  it("Enter sends a plain message and carries no vymalo.send", async () => {
    const { calls } = mount(accepting(), { state: "done" });
    type("echo go on");
    fireEvent.keyDown(box(), { key: "Enter" });
    await waitFor(() => expect(posts(calls)).toHaveLength(1));
    expect(posts(calls)[0]?.forwardedProps).not.toHaveProperty("vymalo.send");
  });

  it("Ctrl+Shift+Enter is not Stop and send when nothing runs", async () => {
    const { calls } = mount(accepting(), { state: "done" });
    type("echo go on");
    await act(async () => {
      fireEvent.keyDown(box(), { key: "Enter", ctrlKey: true, shiftKey: true });
    });
    await new Promise((r) => setTimeout(r, 30));
    expect(posts(calls).every((b) => b.forwardedProps["vymalo.send"] === undefined)).toBe(true);
  });
});

describe("a plain send before the conversation is on screen", () => {
  // the import of a thread opened at its end (ADR 0059) replaces the transcript the runtime holds: a message added before it is
  // lost to it, so the box keeps the message and sends it once the conversation is shown
  const ways: [string, () => void][] = [
    ["the Send button", () => fireEvent.click(send())],
    ["Enter", () => fireEvent.keyDown(box(), { key: "Enter" })],
    [
      "Ctrl+Enter, a form submit that is no key we read",
      () => fireEvent.keyDown(box(), { key: "Enter", ctrlKey: true }),
    ],
    ["a form submit", () => fireEvent.submit(box().closest("form") as HTMLFormElement)],
  ];
  for (const [way, press] of ways) {
    it(`${way} waits for it and then sends the message, once`, async () => {
      const { calls } = mount(accepting(), { state: "done", shown: false });
      type("echo early");
      press();
      await new Promise((r) => setTimeout(r, 30));
      expect(posts(calls)).toHaveLength(0);

      act(() => show());
      await waitFor(() => expect(posts(calls)).toHaveLength(1));
      expect(posts(calls)[0]?.messages[0]).toMatchObject({ role: "user", content: "echo early" });
      expect(posts(calls)[0]?.forwardedProps).not.toHaveProperty("vymalo.send");
      await waitFor(() => expect(box().value).toBe(""));
      await new Promise((r) => setTimeout(r, 30));
      expect(posts(calls)).toHaveLength(1);
    });
  }

  it("keeps the text in the box meanwhile, and the button says that it waits", async () => {
    mount(accepting(), { state: "done", shown: false });
    type("echo early");
    expect(send().getAttribute("aria-busy")).toBeNull();
    fireEvent.click(send());
    await waitFor(() => expect(send().getAttribute("aria-busy")).toBe("true"));
    expect(box().value).toBe("echo early");
    expect(screen.getByRole("status").textContent).toBe(
      "Your message goes out as soon as the conversation is shown.",
    );
    act(() => show());
    await waitFor(() => expect(box().value).toBe(""));
  });

  it("sends what the box holds when it goes out, and nothing when the person emptied it", async () => {
    const { calls } = mount(accepting(), { state: "done", shown: false });
    type("echo one");
    fireEvent.keyDown(box(), { key: "Enter" });
    type("echo one, and two");
    act(() => show());
    await waitFor(() => expect(posts(calls)).toHaveLength(1));
    expect(posts(calls)[0]?.messages[0]).toMatchObject({ content: "echo one, and two" });

    cleanup();
    const second = mount(accepting(), { state: "done", shown: false });
    type("echo never");
    fireEvent.keyDown(box(), { key: "Enter" });
    type("");
    act(() => show());
    await new Promise((r) => setTimeout(r, 60));
    expect(posts(second.calls)).toHaveLength(0);
  });

  it("an empty box holds nothing: Enter before the conversation is shown sends nothing later", async () => {
    const { calls } = mount(accepting(), { state: "done", shown: false });
    fireEvent.keyDown(box(), { key: "Enter" });
    type("echo later");
    act(() => show());
    await new Promise((r) => setTimeout(r, 60));
    expect(posts(calls)).toHaveLength(0);
    expect(box().value).toBe("echo later");
  });

  it("with the conversation on screen a send is the runtime's own, at once", async () => {
    const { calls } = mount(accepting(), { state: "done" });
    type("echo now");
    fireEvent.click(send());
    await waitFor(() => expect(posts(calls)).toHaveLength(1));
    expect(send().getAttribute("aria-busy")).toBeNull();
  });
});

describe("the composer while the agent waits for an answer", () => {
  it("is not the running box: no split menu (the answer is an interrupt's)", () => {
    mount(accepting(), { state: "blocked" });
    type("main");
    expect(screen.queryByRole("button", { name: "Delivery options" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Stop" })).toBeNull();
  });
});
