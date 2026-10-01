import { describe, expect, it } from "vitest";
import { detectPullRequest, locatePullRequest, safeLinkHref } from "./artifact";

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
  it("names the host of a merge request off gitlab.com", () => {
    expect(detectPullRequest("https://evil.example/acme/demo/-/merge_requests/3")?.label).toBe(
      "evil.example/acme/demo!3",
    );
  });
  it("refuses user information and backslashes", () => {
    expect(detectPullRequest("https://x@gitlab.com/g/p/-/merge_requests/1")).toBeUndefined();
    expect(detectPullRequest("https://gitlab.com\\@evil.example/g/p/-/merge_requests/1")).toBe(
      undefined,
    );
  });
  it("ignores other URLs", () => {
    expect(detectPullRequest("https://github.com/o/r/issues/3")).toBeUndefined();
    expect(detectPullRequest("http://github.com/o/r/pull/3")).toBeUndefined();
    expect(detectPullRequest(undefined)).toBeUndefined();
  });
});

describe("safeLinkHref", () => {
  it("keeps absolute http and https URLs", () => {
    expect(safeLinkHref("https://example.com/report.html")).toBe("https://example.com/report.html");
    expect(safeLinkHref("http://example.com/a b")).toBe("http://example.com/a%20b");
  });
  it("rejects script, data and relative URIs", () => {
    expect(safeLinkHref("javascript:alert(document.cookie)")).toBeUndefined();
    expect(safeLinkHref(" JavaScript:alert(1)")).toBeUndefined();
    expect(safeLinkHref("data:text/html,<script>alert(1)</script>")).toBeUndefined();
    expect(safeLinkHref("/api/threads")).toBeUndefined();
    expect(safeLinkHref("not a url")).toBeUndefined();
    expect(safeLinkHref(undefined)).toBeUndefined();
  });
});

describe("locatePullRequest", () => {
  it("reads the host always, the repository and number when the path names them", () => {
    expect(locatePullRequest("https://github.com/acme/demo/pull/12/files")).toEqual({
      host: "github.com",
      repository: "github.com/acme/demo",
      number: 12,
      label: "acme/demo#12",
    });
    expect(locatePullRequest("https://git.example:8443/g/p/-/merge_requests/4")).toEqual({
      host: "git.example:8443",
      repository: "git.example:8443/g/p",
      number: 4,
      label: "git.example:8443/g/p!4",
    });
    expect(locatePullRequest("https://evil.example/x/pull/9")).toEqual({ host: "evil.example" });
    expect(locatePullRequest("https://github.com/acme/demo/pull/12abc")).toEqual({
      host: "github.com",
    });
    expect(locatePullRequest("javascript:alert(1)")).toBeUndefined();
  });
});
