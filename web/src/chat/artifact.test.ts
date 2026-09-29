import { describe, expect, it } from "vitest";
import { detectPullRequest } from "./artifact";

describe("detectPullRequest", () => {
  it("recognises GitHub pull requests", () => {
    expect(detectPullRequest("https://github.com/vymalo/example/pull/1")).toEqual({
      label: "vymalo/example#1",
      href: "https://github.com/vymalo/example/pull/1",
    });
    expect(detectPullRequest("https://github.com/o/r/pull/42/files")?.label).toBe("o/r#42");
  });
  it("recognises GitLab merge requests", () => {
    expect(detectPullRequest("https://gitlab.com/g/sub/p/-/merge_requests/7")?.label).toBe(
      "g/sub/p!7",
    );
  });
  it("ignores other URLs", () => {
    expect(detectPullRequest("https://github.com/o/r/issues/3")).toBeUndefined();
    expect(detectPullRequest("http://github.com/o/r/pull/3")).toBeUndefined();
    expect(detectPullRequest(undefined)).toBeUndefined();
  });
});
