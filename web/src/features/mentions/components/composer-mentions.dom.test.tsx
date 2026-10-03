// @vitest-environment jsdom
import { cleanup, configure, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { type ComponentProps, useSyncExternalStore } from "react";
import { afterEach, describe, expect, it } from "vitest";
import { Composer } from "@/features/chat/components/composer";
import { LiveStream, problem, sse } from "@/features/chat/lib/agui/testing";
import { mountRuntime } from "@/features/chat/lib/agui/testing-runtime";
import type { ThreadAgent } from "@/features/chat/lib/agui/thread-agent";
import type { ApiAgent } from "@/lib/api/types";
import { MentionsStore } from "../lib/store";

configure({ asyncUtilTimeout: 10_000 });
afterEach(cleanup);

globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
};

type Body = {
  runId: string;
  messages: { id: string; role: string; content: string }[];
  forwardedProps: Record<string, unknown>;
};

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

const AGENTS: ApiAgent[] = [
  {
    id: "reviewer",
    name: "Reviewer",
    description: "Reviews a pull request.",
    cardUrl: "http://reviewer/card",
  },
  { id: "researcher", name: "Researcher", cardUrl: "http://researcher/card" },
  { id: "verifier", name: "Verifier" },
];

const failures: string[] = [];
afterEach(() => {
  failures.length = 0;
});

type Props = Partial<ComponentProps<typeof Composer>> & { agents?: ApiAgent[] };

function mount(handler = accepting(), props: Props = {}) {
  const store = new MentionsStore();
  const { agents = AGENTS, state = "done", ...rest } = props;
  const mounted = mountRuntime(handler, { mentions: store }, (agent: ThreadAgent) => (
    <Wired agent={agent} agents={agents} store={store} state={state} {...rest} />
  ));
  mounted.agent.start();
  return { ...mounted, store };
}

function Wired({
  agent,
  agents,
  store,
  state,
  ...rest
}: Props & { agent: ThreadAgent; agents: ApiAgent[]; store: MentionsStore }) {
  const snapshot = useSyncExternalStore(agent.onChange, agent.getSnapshot, agent.getSnapshot);
  return (
    <Composer
      state={state}
      isNew={false}
      sendError={null}
      onCancel={() => {}}
      mentions={{ agents, store }}
      sending={{
        send: (text, mode) => agent.sendWhileWorking(text, mode),
        onFailed: (message) => void failures.push(message),
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
const list = () => screen.queryByRole("listbox", { name: "Agents to mention" });
const options = () => within(list() as HTMLElement).getAllByRole("option");
const chips = () => [...document.querySelectorAll('[data-slot="mention-chip"]')];
const posts = (calls: { method: string; body?: unknown }[]) =>
  calls.filter((c) => c.method === "POST").map((c) => c.body as Body);
const mentionsOf = (body: Body | undefined) => body?.forwardedProps["vymalo.mentions"];

describe("the autocomplete: an ARIA combobox on the box, a listbox above it", () => {
  it("is a textbox with suggestions that becomes a combobox while the list is open", () => {
    mount();
    // closed: the box every screen finds by its name, saying that it has a list
    expect(box().getAttribute("role")).toBeNull();
    expect(screen.getByRole("textbox", { name: "Message" })).toBe(box());
    expect(box().getAttribute("aria-expanded")).toBeNull();
    expect(box().getAttribute("aria-autocomplete")).toBe("list");
    expect(box().getAttribute("aria-haspopup")).toBe("listbox");
    expect(list()).toBeNull();
    type("hello");
    expect(list()).toBeNull();
    type("mail me@exa");
    expect(list()).toBeNull();
    type("ask @");
    expect(list()).not.toBeNull();
    expect(screen.getByRole("combobox", { name: "Message" })).toBe(box());
  });

  it("opens on an @, lists the agents that may be mentioned and points the box at the active one", () => {
    mount();
    type("ask @");
    expect(box().getAttribute("aria-expanded")).toBe("true");
    expect(box().getAttribute("aria-controls")).toBe(list()?.id);
    const all = options();
    expect(all.map((o) => o.getAttribute("data-agent"))).toEqual([
      "reviewer",
      "researcher",
      "verifier",
    ]);
    expect(all[0]?.textContent).toContain("Reviewer");
    expect(all[0]?.textContent).toContain("@reviewer");
    expect(all[0]?.textContent).toContain("Reviews a pull request.");
    // the first is active, and the box says so
    expect(all[0]?.getAttribute("aria-selected")).toBe("true");
    expect(all[1]?.getAttribute("aria-selected")).toBe("false");
    expect(box().getAttribute("aria-activedescendant")).toBe(all[0]?.id);
  });

  it("narrows by what follows the @ and closes when nothing matches", () => {
    mount();
    type("ask @re");
    expect(options().map((o) => o.getAttribute("data-agent"))).toEqual(["reviewer", "researcher"]);
    type("ask @rev");
    expect(options().map((o) => o.getAttribute("data-agent"))).toEqual(["reviewer"]);
    type("ask @zzz");
    expect(list()).toBeNull();
    expect(box().getAttribute("role")).toBeNull();
    expect(box().getAttribute("aria-expanded")).toBeNull();
    expect(box().getAttribute("aria-activedescendant")).toBeNull();
  });

  it("the arrows move the active option (and wrap), the box keeps the focus", () => {
    mount();
    box().focus();
    type("@");
    key("ArrowDown");
    expect(box().getAttribute("aria-activedescendant")).toBe(options()[1]?.id);
    key("ArrowDown");
    key("ArrowDown"); // wraps to the first
    expect(box().getAttribute("aria-activedescendant")).toBe(options()[0]?.id);
    key("ArrowUp"); // and back to the last
    expect(box().getAttribute("aria-activedescendant")).toBe(options()[2]?.id);
    expect(options()[2]?.getAttribute("aria-selected")).toBe("true");
    expect(document.activeElement).toBe(box());
  });

  it("Enter picks the active agent: its label goes into the text, a chip appears, the list closes", () => {
    mount();
    type("ask @");
    key("ArrowDown");
    key("Enter");
    expect(box().value).toBe("ask @researcher ");
    expect(list()).toBeNull();
    expect(chips().map((c) => c.getAttribute("data-agent"))).toEqual(["researcher"]);
    const chip = screen.getByRole("list", { name: "Mentioned agents" });
    expect(
      within(chip).getByRole("button", { name: "Remove the mention of Researcher" }),
    ).toBeTruthy();
  });

  it("Tab picks too, and a click on an option does", () => {
    mount();
    type("@ver");
    key("Tab");
    expect(box().value).toBe("@verifier ");
    type("@verifier then @");
    fireEvent.mouseDown(options()[0] as HTMLElement); // does not take the focus from the box
    fireEvent.click(options()[0] as HTMLElement);
    expect(box().value).toBe("@verifier then @reviewer ");
    expect(chips()).toHaveLength(2);
  });

  it("Escape closes the list until the word changes; Enter then is not a pick", () => {
    mount();
    type("ask @re");
    key("Escape");
    expect(list()).toBeNull();
    expect(box().value).toBe("ask @re");
    type("ask @rev");
    expect(list()).not.toBeNull();
  });

  it("an agent already mentioned is not offered again", () => {
    mount();
    type("@rev");
    key("Enter");
    type("@reviewer and @");
    expect(options().map((o) => o.getAttribute("data-agent"))).toEqual(["researcher", "verifier"]);
  });

  it("a key with Ctrl, Cmd or Alt, and a key in an IME composition, are not the list's", () => {
    mount();
    type("@");
    key("Tab", { ctrlKey: true });
    key("Tab", { altKey: true });
    key("Enter", { shiftKey: true });
    key("Enter", { isComposing: true });
    expect(box().value).toBe("@");
    expect(chips()).toHaveLength(0);
  });

  it("opens nothing, and says nothing of suggestions, when no agent can be mentioned", () => {
    mount(accepting(), { agents: [] });
    expect(box().getAttribute("aria-haspopup")).toBeNull();
    type("ask @");
    expect(list()).toBeNull();
  });
});

describe("the mentions follow the text", () => {
  it("typing in front moves the offsets; the chip stays", async () => {
    const { calls } = mount();
    type("ask @");
    key("Enter");
    type("please ask @reviewer ");
    expect(chips()).toHaveLength(1);
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    await waitFor(() => expect(posts(calls)).toHaveLength(1));
    expect(mentionsOf(posts(calls)[0])).toEqual([
      {
        agentId: "reviewer",
        label: "@reviewer",
        start: 11,
        end: 20,
        cardUrl: "http://reviewer/card",
      },
    ]);
  });

  it("editing the label drops the mention: no chip, no member in the run", async () => {
    const { calls } = mount();
    type("ask @");
    key("Enter");
    expect(chips()).toHaveLength(1);
    type("ask @reviewe ");
    expect(chips()).toHaveLength(0);
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    await waitFor(() => expect(posts(calls)).toHaveLength(1));
    expect(posts(calls)[0]?.forwardedProps).not.toHaveProperty("vymalo.mentions");
    expect(posts(calls)[0]?.messages[0]?.content).toBe("ask @reviewe ");
  });

  it("the remove button takes the label out of the text with the mention", () => {
    mount();
    type("ask @");
    key("Enter");
    type("ask @reviewer to plot");
    fireEvent.click(screen.getByRole("button", { name: "Remove the mention of Reviewer" }));
    expect(box().value).toBe("ask to plot");
    expect(chips()).toHaveLength(0);
  });

  it("emptying the box drops them all", () => {
    mount();
    type("@");
    key("Enter");
    expect(chips()).toHaveLength(1);
    type("");
    expect(chips()).toHaveLength(0);
  });
});

describe("what a message carries", () => {
  it("offsets in UTF-16 code units: an emoji, a combining mark and two mentions", async () => {
    const { calls } = mount();
    const accent = String.fromCharCode(0x65, 0x301);
    type(`😄 ${accent} @`);
    key("Enter");
    type(`😄 ${accent} @reviewer and @`);
    // the first agent not yet mentioned is the researcher
    key("Enter");
    const text = `😄 ${accent} @reviewer and @researcher `;
    expect(box().value).toBe(text);
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    await waitFor(() => expect(posts(calls)).toHaveLength(1));
    const body = posts(calls)[0];
    const sent = body?.messages[0]?.content ?? "";
    const refs = mentionsOf(body) as { label: string; start: number; end: number }[];
    expect(refs).toHaveLength(2);
    // what the orchestrator checks: the label is the text at the offsets, in code units
    for (const r of refs) expect(sent.slice(r.start, r.end)).toBe(r.label);
    expect(refs[0]?.start).toBe("😄 ".length + accent.length + 1);
    expect(refs[0]?.start).toBe(6); // the emoji (2), a space, the letter and its mark (2), a space
    expect(refs.map((r) => r.label)).toEqual(["@reviewer", "@researcher"]);
    expect(sent.startsWith("😄")).toBe(true);
  });

  it("the box and its chips are empty once the message is on its way", async () => {
    const { calls, store } = mount();
    type("@");
    key("Enter");
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    await waitFor(() => expect(posts(calls)).toHaveLength(1));
    expect(mentionsOf(posts(calls)[0])).toHaveLength(1);
    await waitFor(() => expect(box().value).toBe(""));
    expect(chips()).toHaveLength(0);
    expect(store.current).toEqual([]);
  });

  it("Enter sends with the mentions", async () => {
    const { calls } = mount();
    type("see @");
    key("Enter"); // a pick, not a send
    expect(posts(calls)).toHaveLength(0);
    key("Enter"); // the second Enter sends
    await waitFor(() => expect(posts(calls)).toHaveLength(1));
    expect(mentionsOf(posts(calls)[0])).toMatchObject([{ agentId: "reviewer", start: 4, end: 13 }]);
  });

  it("a message with leading white space goes out as the runtime sends it, the offsets matching its text", async () => {
    const { calls } = mount();
    type("  \n @");
    key("Enter");
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    await waitFor(() => expect(posts(calls)).toHaveLength(1));
    const body = posts(calls)[0];
    const sent = body?.messages[0]?.content ?? "";
    const [ref] = mentionsOf(body) as { label: string; start: number; end: number }[];
    expect(sent.slice(ref?.start, ref?.end)).toBe("@reviewer");
  });

  it("while the agent works, Send keeps the mentions, with vymalo.send", async () => {
    const { calls } = mount(accepting(), { state: "working" });
    type("steer @");
    key("ArrowDown");
    key("Enter");
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    await waitFor(() => expect(posts(calls)).toHaveLength(1));
    const body = posts(calls)[0];
    expect(body?.forwardedProps["vymalo.send"]).toBe("steer");
    expect(mentionsOf(body)).toEqual([
      {
        agentId: "researcher",
        label: "@researcher",
        start: 6,
        end: 17,
        cardUrl: "http://researcher/card",
      },
    ]);
  });

  it("while the agent works, Stop and send keeps them, and Enter with the list open is a pick, not a send", async () => {
    const { calls } = mount(accepting(), { state: "working" });
    type("redo @");
    key("Enter"); // picks
    expect(posts(calls)).toHaveLength(0);
    key("Enter", { ctrlKey: true, shiftKey: true });
    await waitFor(() => expect(posts(calls)).toHaveLength(1));
    expect(posts(calls)[0]?.forwardedProps["vymalo.send"]).toBe("interrupt");
    expect(mentionsOf(posts(calls)[0])).toMatchObject([{ agentId: "reviewer", start: 5, end: 14 }]);
  });
});

describe("a refused send keeps the text with its mentions in the box", () => {
  const refusing = (detail: string, status = 422) => {
    const connect = new LiveStream();
    return (call: { method: string }) =>
      call.method === "GET" ? sse(connect.body) : problem(status, "Unprocessable", detail);
  };

  it("a message of an idle thread (the runtime's send): the text and the chip come back", async () => {
    const { calls, box: runtimeBox } = mount(refusing("unknown agent 'reviewer' in mentions"));
    type("ask @");
    key("Enter");
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    await waitFor(() => expect(posts(calls)).toHaveLength(1));
    await waitFor(() => expect(runtimeBox.errors.length).toBeGreaterThan(0));
    await waitFor(() => expect(box().value).toBe("ask @reviewer "));
    await waitFor(() => expect(chips()).toHaveLength(1));
    // sent again, it carries the same reference
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    await waitFor(() => expect(posts(calls)).toHaveLength(2));
    expect(mentionsOf(posts(calls)[1])).toEqual(mentionsOf(posts(calls)[0]));
  });

  it("a message sent while the agent works: the text and the chip come back, in front of what was written since", async () => {
    const { calls } = mount(refusing("the card of 'reviewer' moved; refresh the agent list"), {
      state: "working",
    } as Props);
    type("ask @");
    key("Enter");
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    type("and more");
    await waitFor(() =>
      expect(failures).toEqual(["the card of 'reviewer' moved; refresh the agent list"]),
    );
    await waitFor(() => expect(box().value).toBe("ask @reviewer\n\nand more"));
    await waitFor(() => expect(chips()).toHaveLength(1));
    expect(posts(calls)).toHaveLength(1);
    // and it still stands: sent again, the offsets are those of the restored text
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    await waitFor(() => expect(posts(calls)).toHaveLength(2));
    const second = posts(calls)[1];
    const [ref] = mentionsOf(second) as { label: string; start: number; end: number }[];
    expect(second?.messages[0]?.content.slice(ref?.start, ref?.end)).toBe("@reviewer");
  });
});
