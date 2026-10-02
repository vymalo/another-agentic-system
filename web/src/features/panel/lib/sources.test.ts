import { describe, expect, it } from "vitest";
import {
  agentTurns,
  collectSources,
  countSources,
  linksIn,
  normaliseUrl,
  type SourceMessage,
} from "./sources";

const PR = "https://github.com/acme/demo/pull/12";

const text = (value: string) => ({ type: "text", text: value });
const data = (activity: string, content: Record<string, unknown>) => ({
  type: "data",
  name: `agui-activity/vymalo.${activity}`,
  data: content,
});
const artifact = (content: Record<string, unknown>) => data("artifact", content);
const ci = (content: Record<string, unknown> = {}) =>
  data("ci", {
    name: "ci/build",
    conclusion: "failure",
    passed: false,
    sha: "a".repeat(40),
    shortSha: "aaaaaaa",
    provider: "github",
    repository: "github.com/acme/demo",
    url: "https://ci.example.com/runs/1",
    ...content,
  });
const agent = (id: string, ...content: SourceMessage["content"][number][]): SourceMessage => ({
  id,
  role: "assistant",
  content,
});
const person = (id: string, value: string): SourceMessage => ({
  id,
  role: "user",
  content: [text(value)],
});

const pullRequest = artifact({
  kind: "pull_request",
  name: "pull_request",
  url: PR,
  repository: "github.com/acme/demo",
  number: 12,
  text: JSON.stringify({ title: "Fix the redirect loop" }),
});

/** The items of every group, flat. */
const items = (messages: SourceMessage[]) => collectSources(messages).flatMap((g) => g.items);

describe("normaliseUrl", () => {
  it("lower-cases the host, drops a default port and a trailing slash, keeps query and fragment", () => {
    expect(normaliseUrl("https://Example.COM:443/a/b/")).toBe("https://example.com/a/b");
    expect(normaliseUrl("https://example.com/")).toBe("https://example.com/");
    expect(normaliseUrl("https://example.com/a?x=1#top")).toBe("https://example.com/a?x=1#top");
    expect(normaliseUrl("http://example.com:8080/a")).toBe("http://example.com:8080/a");
  });

  it("keeps different fragments, queries and schemes apart", () => {
    const keys = new Set([
      normaliseUrl("https://e.example/a#x"),
      normaliseUrl("https://e.example/a#y"),
      normaliseUrl("https://e.example/a?q=1"),
      normaliseUrl("http://e.example/a"),
    ]);
    expect(keys.size).toBe(4);
  });
});

describe("linksIn", () => {
  it("finds inline links, autolinks, angle-bracket links and reference links", () => {
    const md = [
      "See [the docs](https://docs.rs/axum) and https://example.com/a.",
      "Also <https://angle.example/x> and [the spec][s].",
      "",
      "[s]: https://spec.example/v1",
    ].join("\n");
    expect(linksIn(md)).toEqual([
      { href: "https://docs.rs/axum", text: "the docs" },
      { href: "https://example.com/a", text: "https://example.com/a" },
      { href: "https://angle.example/x", text: "https://angle.example/x" },
      { href: "https://spec.example/v1", text: "the spec" },
    ]);
  });

  it("never reads a URL in code: an inline span, an indented block, a fence", () => {
    const md = [
      "Run `curl https://code.example/span` first.",
      "",
      "```sh",
      "curl https://code.example/fence",
      "```",
      "",
      "    curl https://code.example/indented",
    ].join("\n");
    expect(linksIn(md)).toEqual([]);
  });

  it("refuses every target that is not an absolute http(s) URL", () => {
    const md = [
      "[a](javascript:alert(1)) [b](data:text/html,<b>x</b>) [c](/relative) [d](//proto.example)",
      "[e](mailto:me@example.com) [f](ftp://f.example/x) [g](https://user@evil.example/) [h](#frag)",
    ].join("\n");
    expect(linksIn(md)).toEqual([]);
  });

  it("accepts http, and reads the words of a link with markup in them", () => {
    expect(linksIn("[**bold** `code`](http://plain.example/)")).toEqual([
      { href: "http://plain.example/", text: "bold code" },
    ]);
  });

  it("does not take an image for a link, but a link round an image gets its description", () => {
    expect(linksIn("![logo](https://img.example/a.png)")).toEqual([]);
    expect(linksIn("[![the logo](https://img.example/a.png)](https://home.example/)")).toEqual([
      { href: "https://home.example/", text: "the logo" },
    ]);
  });

  it("says each URL once per text, and stops at a hundred", () => {
    expect(linksIn("https://a.example/ https://a.example https://b.example")).toHaveLength(2);
    const many = Array.from({ length: 150 }, (_, i) => `https://e.example/${i}`).join(" ");
    expect(linksIn(many)).toHaveLength(100);
  });

  it("is nothing for a text with no URL in it", () => {
    expect(linksIn("no links here, just words [and brackets](not-a-url)")).toEqual([]);
  });
});

describe("collectSources: what each kind of source is", () => {
  it("a pull request: its label and title, its host, its link", () => {
    expect(items([agent("a1", pullRequest)])).toEqual([
      {
        key: "https://github.com/acme/demo/pull/12",
        kind: "pull_request",
        title: "acme/demo#12 — Fix the redirect loop",
        detail: "github.com",
        href: PR,
        turns: [{ id: "a1", number: 1 }],
      },
    ]);
  });

  it("a file whose link is a pull request is a pull request, with no title of its own", () => {
    const [pr] = items([agent("a1", artifact({ name: "result", text: "echo: hi", uri: PR }))]);
    expect(pr).toMatchObject({ kind: "pull_request", title: "acme/demo#12", href: PR });
  });

  it("a pull request on another host is labelled with the host", () => {
    const [pr] = items([
      agent(
        "a1",
        artifact({
          kind: "pull_request",
          name: "pull_request",
          url: "https://git.example.org/acme/demo/pull/9",
        }),
      ),
    ]);
    expect(pr?.title).toBe("git.example.org/acme/demo#9");
  });

  it("a branch is text: with no link unless its repository really is an https URL", () => {
    const [plain] = items([
      agent(
        "a1",
        artifact({
          kind: "branch",
          name: "branch",
          repository: "github.com/acme/demo",
          branch: "agent/fix",
          sha: "a".repeat(40),
        }),
      ),
    ]);
    expect(plain).toEqual({
      key: "branch:github.com/acme/demo#agent/fix",
      kind: "branch",
      title: "agent/fix · acme/demo",
      turns: [{ id: "a1", number: 1 }],
    });
    expect(plain && "href" in plain).toBe(false);

    const [linked] = items([
      agent(
        "a1",
        artifact({
          kind: "branch",
          name: "branch",
          repository: "https://github.com/acme/demo",
          branch: "agent/fix",
        }),
      ),
    ]);
    expect(linked?.href).toBe("https://github.com/acme/demo");
  });

  it("a branch without its repository or its name, and a checks artifact, are no source", () => {
    expect(
      items([
        agent(
          "a1",
          artifact({ kind: "branch", name: "branch", branch: "x" }),
          artifact({ kind: "checks", name: "checks", passed: true }),
        ),
      ]),
    ).toEqual([]);
  });

  it("a file the store kept (ADR 0032): listed by its hash, opened and downloaded from the API's route", () => {
    const sha = "9".repeat(64);
    const href = `/api/threads/t-1/artifacts/${sha}`;
    const file = (over: Record<string, unknown> = {}) =>
      artifact({
        kind: "file",
        name: "chart",
        mimeType: "image/png",
        href,
        sha256: sha,
        size: 2048,
        filename: "chart.png",
        preview: "image",
        ...over,
      });
    const found = items([agent("a1", file()), agent("a2", file({ filename: "again.png" }))]);
    // one file, cited by two turns
    expect(found).toHaveLength(1);
    expect(found[0]).toMatchObject({
      key: `file:${sha}`,
      kind: "file",
      title: "chart.png",
      detail: "2.0 KB · image/png",
      href,
      downloadHref: `${href}?download=1`,
    });
    expect(found[0]?.turns.map((t) => t.number)).toEqual([1, 2]);
    // an attachment-only file is a source too
    expect(items([agent("a1", file({ preview: null, mimeType: "application/zip" }))])).toHaveLength(
      1,
    );
    // a reference that is not the API's route is no file at all (and its uri is not followed)
    expect(
      items([
        agent("a1", file({ href: "https://evil.example/x.png", uri: "javascript:alert(1)" })),
      ]),
    ).toEqual([]);
  });

  it("a file with an http(s) link: its name and type; any other link is no source", () => {
    const [file] = items([
      agent(
        "a1",
        artifact({
          name: "report.md",
          mimeType: "text/markdown",
          uri: "https://files.example/report.md",
        }),
      ),
    ]);
    expect(file).toMatchObject({
      kind: "file",
      title: "report.md",
      detail: "text/markdown",
      href: "https://files.example/report.md",
    });
    for (const uri of [
      "javascript:alert(1)",
      "data:text/plain,hi",
      "/etc/passwd",
      "file:///x",
      "",
    ]) {
      expect(items([agent("a1", artifact({ name: "f", uri }))]), uri).toEqual([]);
    }
    expect(items([agent("a1", artifact({ name: "f", text: "inline only" }))])).toEqual([]);
  });

  it("a CI report with a link: the check, how it ended, the provider", () => {
    expect(items([agent("a1", ci())])).toEqual([
      {
        key: "https://ci.example.com/runs/1",
        kind: "ci",
        title: "CI: ci/build — failure",
        detail: "GitHub",
        href: "https://ci.example.com/runs/1",
        passed: false,
        turns: [{ id: "a1", number: 1 }],
      },
    ]);
    const [green] = items([agent("a1", ci({ conclusion: "success", passed: true }))]);
    expect(green).toMatchObject({ title: "CI: ci/build — passed", passed: true });
  });

  it("a CI report without a usable link is no source", () => {
    expect(items([agent("a1", ci({ url: undefined }))])).toEqual([]);
    expect(items([agent("a1", ci({ url: "javascript:alert(1)" }))])).toEqual([]);
    expect(items([agent("a1", ci({ url: "ftp://ci.example.com/1" }))])).toEqual([]);
  });

  it("links in the agent's words: the text or, with none of its own, the host and path", () => {
    const found = items([
      agent(
        "a1",
        text("See [the axum docs](https://docs.rs/axum/latest) and https://example.com/a/"),
      ),
    ]);
    expect(found.map((s) => [s.kind, s.title, s.detail])).toEqual([
      ["link", "the axum docs", "docs.rs"],
      ["link", "example.com/a", "example.com"],
    ]);
  });

  it("only the agents' words and artifacts count, never the person's", () => {
    expect(items([person("u1", "look at https://mine.example/")])).toEqual([]);
    expect(
      items([
        person("u1", "look at https://mine.example/"),
        agent("a1", text("ok https://x.example/")),
      ]),
    ).toHaveLength(1);
  });
});

describe("collectSources: turns, order and groups", () => {
  it("numbers the turns as the chat does: a message that draws nothing is no turn", () => {
    const messages = [
      person("u1", "go"),
      agent("a1", data("status", { status: "completed" })), // says nothing the chat draws
      agent("a2", text("done: https://x.example/")),
    ];
    expect(agentTurns(messages)).toEqual([{ id: "a2", number: 1 }]);
    expect(items(messages)[0]?.turns).toEqual([{ id: "a2", number: 1 }]);
  });

  it("is one item per URL however often it is cited, with every turn that cited it", () => {
    const found = items([
      agent("a1", text("see https://Example.com/a/")),
      person("u1", "and?"),
      agent("a2", text("again [here](https://example.com/a)"), text("and https://example.com/a")),
    ]);
    expect(found).toHaveLength(1);
    expect(found[0]?.turns).toEqual([
      { id: "a1", number: 1 },
      { id: "a2", number: 2 },
    ]);
    // the words are those of the first citation
    expect(found[0]?.title).toBe("example.com/a");
  });

  it("a typed source takes the place of the same URL in someone's words, where it first appeared", () => {
    const found = items([
      agent("a1", text(`I will open ${PR} and https://x.example/`)),
      agent("a2", pullRequest),
    ]);
    expect(found.map((s) => [s.kind, s.key])).toEqual([
      ["pull_request", "https://github.com/acme/demo/pull/12"],
      ["link", "https://x.example/"],
    ]);
    expect(found[0]?.turns.map((t) => t.number)).toEqual([1, 2]);
  });

  it("a branch pushed again by a rework is one item", () => {
    const branch = artifact({
      kind: "branch",
      name: "branch",
      repository: "github.com/acme/demo",
      branch: "agent/fix",
    });
    const found = items([agent("a1", branch), agent("a2", branch)]);
    expect(found).toHaveLength(1);
    expect(found[0]?.turns).toHaveLength(2);
  });

  it("groups them as pull requests and branches, checks, files, links, in the order they appeared", () => {
    const groups = collectSources([
      agent(
        "a1",
        text("https://first.example/"),
        artifact({ name: "r.md", uri: "https://files.example/r.md" }),
        ci(),
        pullRequest,
        artifact({ kind: "branch", name: "branch", repository: "github.com/a/b", branch: "x" }),
        text("https://second.example/"),
      ),
    ]);
    expect(groups.map((g) => [g.id, g.label, g.items.map((s) => s.kind)])).toEqual([
      ["code", "Pull requests & branches", ["pull_request", "branch"]],
      ["checks", "Checks", ["ci"]],
      ["files", "Files", ["file"]],
      ["links", "Links", ["link", "link"]],
    ]);
    expect(countSources(groups)).toBe(6);
  });

  it("leaves out the groups with nothing in them, and is empty for a thread with nothing shared", () => {
    expect(collectSources([agent("a1", text("only words"))])).toEqual([]);
    expect(collectSources([])).toEqual([]);
    expect(collectSources([agent("a1", text("[x](https://x.example/)"))]).map((g) => g.id)).toEqual(
      ["links"],
    );
  });

  it("follows a text as it streams: the same message with more words", () => {
    const first = collectSources([agent("a1", text("see https://a.example/"))]);
    expect(countSources(first)).toBe(1);
    const grown = collectSources([
      agent("a1", text("see https://a.example/ and https://b.example/")),
    ]);
    expect(countSources(grown)).toBe(2);
  });

  it("never turns a link's words into markup: raw HTML in them is left out, text stays text", () => {
    const [html] = items([agent("a1", text("[<img src=x onerror=alert(1)>](https://x.example/)"))]);
    expect(html?.title).toBe("x.example");
    const [mixed] = items([agent("a1", text("[a & <b>c</b> *d*](https://x.example/)"))]);
    expect(mixed?.title).not.toContain("<");
    expect(mixed?.title).toBe("a & c d");
  });
});
