import { describe, expect, it } from "vitest";
import { repoKey, typedArtifact } from "./projection";

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
