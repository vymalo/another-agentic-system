import { describe, expect, it } from "vitest";
import { CatalogLedger, Projector, repoKey, typedArtifact } from "./projection";

/**
 * The typed artifact fields of the mock's projection, on the same cases as the real one's
 * `an_artifact_says_its_kind_and_the_fields_a_card_needs` (orch-agui-projection) and
 * `a_pull_request_is_recognised_from_either_agent_shape` (orch-core): the goldens only hold
 * `branch`, `checks` and `file` artifacts.
 */
const SHA = "0123456789abcdef0123456789abcdef01234567";

describe("typedArtifact", () => {
  it("reads a branch, the agent's checks and a pull request in either agent shape", () => {
    expect(
      typedArtifact(
        "branch",
        undefined,
        JSON.stringify({
          repository: "https://github.com/Acme/Demo.git",
          branch: "agent/fix",
          commit: SHA,
        }),
      ),
    ).toEqual({
      kind: "branch",
      repository: "github.com/acme/demo",
      branch: "agent/fix",
      sha: SHA,
      shortSha: "0123456",
    });
    expect(
      typedArtifact(
        "checks",
        undefined,
        JSON.stringify({ passed: false, commit: SHA, findings: ["x"] }),
      ),
    ).toEqual({ kind: "checks", passed: false, sha: SHA, shortSha: "0123456" });
    expect(
      typedArtifact(
        "pull_request",
        undefined,
        JSON.stringify({
          url: "https://github.com/acme/demo/pull/12",
          number: "12",
          branch: "agent/fix",
        }),
      ),
    ).toEqual({
      kind: "pull_request",
      url: "https://github.com/acme/demo/pull/12",
      number: 12,
      repository: "github.com/acme/demo",
      branch: "agent/fix",
    });
    expect(typedArtifact("Pull request", "https://github.com/acme/demo/pull/13", "opened")).toEqual(
      {
        kind: "pull_request",
        url: "https://github.com/acme/demo/pull/13",
        number: 13,
        repository: "github.com/acme/demo",
      },
    );
    expect(
      typedArtifact("pull-request", "https://gitlab.com/acme/demo/-/merge_requests/7", undefined),
    ).toMatchObject({
      number: 7,
      repository: "gitlab.com/acme/demo",
    });
  });

  it("makes a file of anything else, and of a pull request or branch it cannot use", () => {
    expect(typedArtifact("branch", undefined, "not json")).toEqual({ kind: "file" });
    expect(
      typedArtifact(
        "checks",
        undefined,
        JSON.stringify({ passed: true, commit: SHA, findings: "x" }),
      ),
    ).toEqual({
      kind: "file",
    });
    expect(typedArtifact("result", "https://github.com/acme/demo/pull/1", undefined)).toEqual({
      kind: "file",
    });
    for (const bad of [
      "http://github.com/a/b/pull/1",
      "javascript:alert(1)",
      "https://",
      "https://github.com/a b",
      "https://github.com@evil.example/acme/demo/pull/1",
      "https://github.com\\@evil.example/acme/demo/pull/1",
      "https://@/x",
    ]) {
      expect(typedArtifact("Pull request", bad, undefined)).toEqual({ kind: "file" });
    }
  });

  it("takes the repository and the number from the URL, never from the payload alone", () => {
    const pr = (url: string) =>
      typedArtifact(
        "pull_request",
        undefined,
        JSON.stringify({ url, repository: "github.com/acme/demo", number: 12 }),
      );
    expect(pr("https://evil.example/acme/demo/pull/9")).toEqual({
      kind: "pull_request",
      url: "https://evil.example/acme/demo/pull/9",
      number: 9,
      repository: "evil.example/acme/demo",
    });
    expect(pr("https://evil.example/x/pull/9")).toEqual({
      kind: "pull_request",
      url: "https://evil.example/x/pull/9",
    });
    expect(pr("https://evil.example/review")).toEqual({
      kind: "pull_request",
      url: "https://evil.example/review",
    });
  });

  it("normalises a repository like the core", () => {
    expect(repoKey("git@github.com:Vymalo/Repo.git")).toBe("github.com/vymalo/repo");
    expect(repoKey("https://github.com:443/vymalo/repo")).toBe("github.com/vymalo/repo");
    expect(repoKey("ssh://git@host:2222/a/b")).toBe("host:2222/a/b");
    expect(repoKey("https://github.com/only")).toBeUndefined();
    expect(repoKey("https://github.com/a/../b")).toBeUndefined();
  });
});

/**
 * The UI catalog ledger (ADR 0023), on the same cases as the real one's (`UiCatalogLedger`, in
 * `orch-core`): a digest is recorded once, the highest version is current, an older one is
 * recorded and never current, the same version with another digest replaces the current one.
 */
describe("CatalogLedger", () => {
  const ref = (version: number, tag: string) => ({
    catalogId: "https://agents.vymalo.com/a2ui/catalogs/chat",
    version,
    digest: `sha256:${tag.repeat(64)}`,
  });

  it("records a digest once and keeps the highest version current", () => {
    const ledger = new CatalogLedger();
    expect(ledger.observe(ref(1, "a"))).toEqual({ recorded: true, becameCurrent: true });
    expect(ledger.observe(ref(1, "a"))).toEqual({ recorded: false, becameCurrent: false });
    expect(ledger.observe(ref(2, "b"))).toEqual({ recorded: true, becameCurrent: true });
    expect(ledger.observe(ref(1, "c"))).toEqual({ recorded: true, becameCurrent: false });
    expect(ledger.observe(ref(1, "c"))).toEqual({ recorded: false, becameCurrent: false });
    expect(ledger.current).toEqual(ref(2, "b"));
    expect(ledger.knows(ref(1, "c").digest)).toBe(true);
    expect(ledger.knows(ref(9, "d").digest)).toBe(false);
  });

  it("lets the later of two digests of one version win, once", () => {
    const ledger = new CatalogLedger();
    ledger.observe(ref(2, "a"));
    expect(ledger.observe(ref(2, "b"))).toEqual({ recorded: true, becameCurrent: true });
    expect(ledger.observe(ref(2, "a"))).toEqual({ recorded: false, becameCurrent: false });
    expect(ledger.current).toEqual(ref(2, "b"));
  });
});

describe("Projector and the UI catalog", () => {
  const info = {
    threadId: "00000000-0000-7000-8000-000000000001",
    title: "t",
    target: { agentId: "plain" },
  };
  const at = "2026-01-01T00:00:00Z";
  const user = { type: "user" as const, name: "alice@example.com" };
  const catalogEvent = (seq: number, version: number, tag: string) => ({
    seq,
    threadId: info.threadId,
    at,
    kind: "ui_catalog" as const,
    actor: user,
    data: {
      catalogId: "https://agents.vymalo.com/a2ui/catalogs/chat",
      version,
      digest: `sha256:${tag.repeat(64)}`,
      catalog: { catalogId: "https://agents.vymalo.com/a2ui/catalogs/chat", components: {} },
    },
  });
  const message = (seq: number) => ({
    seq,
    threadId: info.threadId,
    at,
    kind: "user_message" as const,
    actor: user,
    data: { text: "hi", messageId: `m-${seq}`, runId: `r-${seq}` },
  });

  it("says nothing for a catalog, opens no run, and puts the current one in the snapshot", () => {
    const projector = new Projector(info);
    expect(projector.apply(catalogEvent(1, 1, "a"))).toEqual([]);
    expect(projector.runOpen).toBe(false);
    const frames = projector.apply(message(2));
    expect(frames[0]?.event.type).toBe("RUN_STARTED");
    const snapshot = frames.find((f) => f.event.type === "STATE_SNAPSHOT")?.event as unknown as {
      snapshot: { thread: Record<string, unknown> };
    };
    expect(snapshot.snapshot.thread.uiCatalog).toEqual({
      catalogId: "https://agents.vymalo.com/a2ui/catalogs/chat",
      version: 1,
      digest: `sha256:${"a".repeat(64)}`,
    });
    // in the middle of a run: still nothing, and the newest version is the one the snapshot names
    expect(projector.apply(catalogEvent(3, 2, "b"))).toEqual([]);
    expect(projector.runOpen).toBe(true);
    expect(projector.apply(catalogEvent(4, 1, "c"))).toEqual([]);
  });

  it("leaves a thread that was shown no catalog with no uiCatalog", () => {
    const projector = new Projector(info);
    const frames = projector.apply(message(1));
    const snapshot = frames.find((f) => f.event.type === "STATE_SNAPSHOT")?.event as unknown as {
      snapshot: { thread: Record<string, unknown> };
    };
    expect("uiCatalog" in snapshot.snapshot.thread).toBe(false);
  });
});

describe("the mentions of a user message (ADR 0026)", () => {
  const info = {
    threadId: "00000000-0000-7000-8000-000000000001",
    title: "t",
    target: { agentId: "plain" },
  };
  const at = "2026-01-01T00:00:00Z";
  const user = { type: "user" as const, name: "alice@example.com" };
  const mentions = [
    {
      agentId: "mock-researcher",
      label: "@researcher",
      start: 3,
      end: 14,
      cardUrl: "http://mock-researcher:8080/.well-known/agent-card.json",
    },
    { agentId: "mock-coder", label: "@coder", start: 20, end: 26 },
  ];
  const message = (data: Record<string, unknown>) => ({
    seq: 1,
    threadId: info.threadId,
    at,
    kind: "user_message" as const,
    actor: user,
    data: { text: "\u{1F604} @researcher then @coder", messageId: "m-1", ...data },
  });
  const startOf = (frames: ReturnType<Projector["apply"]>) =>
    frames.find((f) => f.event.type === "TEXT_MESSAGE_START")?.event as unknown as {
      role: string;
      metadata: Record<string, unknown>;
    };

  it("puts the references on the START of the message, beside the actor, as the log stores them", () => {
    const start = startOf(new Projector(info).apply(message({ mentions })));
    expect(start.role).toBe("user");
    expect(start.metadata["vymalo.mentions"]).toEqual(mentions);
    expect(start.metadata["vymalo.actor"]).toEqual({ type: "user", name: user.name });
  });

  it("says nothing of mentions for a message that mentions nobody", () => {
    for (const data of [{}, { mentions: [] }]) {
      const start = startOf(new Projector(info).apply(message(data)));
      expect("vymalo.mentions" in start.metadata).toBe(false);
    }
  });

  it("a message sent while the agent works carries both its delivery and its mentions (ADR 0036)", () => {
    // the first message opens the run, the steer lands in it (and ends it: a message of its own)
    const projector = new Projector(info);
    projector.apply({ ...message({ text: "go" }), seq: 1 });
    const steered = projector.apply({
      ...message({ mentions, delivery: "steer", messageId: "m-2" }),
      seq: 2,
    });
    const start = startOf(steered);
    expect(start.metadata["vymalo.delivery"]).toBe("steer");
    expect(start.metadata["vymalo.mentions"]).toEqual(mentions);
    expect(start.metadata["vymalo.actor"]).toEqual({ type: "user", name: user.name });
    // each member only when the log has it
    const plain = new Projector(info);
    plain.apply({ ...message({ text: "go" }), seq: 1 });
    const onlyDelivery = startOf(
      plain.apply({ ...message({ delivery: "steer", messageId: "m-3" }), seq: 2 }),
    );
    expect(onlyDelivery.metadata["vymalo.delivery"]).toBe("steer");
    expect("vymalo.mentions" in onlyDelivery.metadata).toBe(false);
  });

  it("counts the offsets in UTF-16 code units, which is what a string of this runtime indexes", () => {
    const text = "\u{1F604} @researcher then @coder";
    for (const m of mentions) expect(text.slice(m.start, m.end)).toBe(m.label);
  });
});
