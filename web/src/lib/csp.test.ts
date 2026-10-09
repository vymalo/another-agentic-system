import { createHash } from "node:crypto";
import { describe, expect, it } from "vitest";
import { connectSources, headerPolicy, inlineScripts, metaPolicy, metaTag } from "./csp";

const directive = (policy: string, name: string): string[] =>
  (policy.split("; ").find((d) => d.startsWith(`${name} `)) ?? "").split(" ").slice(1);

describe("the header half of the policy (the static server's)", () => {
  const policy = headerPolicy(["https://id.example", "https://*.id.example:8443"]);

  it("lets the page connect to itself and to the issuer, and post a form to them", () => {
    expect(directive(policy, "connect-src")).toEqual([
      "'self'",
      "https://id.example",
      "https://*.id.example:8443",
    ]);
    expect(directive(policy, "form-action")).toEqual([
      "'self'",
      "https://id.example",
      "https://*.id.example:8443",
    ]);
  });

  it("allows the page's own scripts and inline ones, which the meta half narrows to their hashes; never eval", () => {
    expect(directive(policy, "script-src")).toEqual(["'self'", "'unsafe-inline'"]);
    expect(policy).not.toContain("unsafe-eval");
  });

  it("draws images from this origin, data and blob (the object URLs of kept files)", () => {
    expect(directive(policy, "img-src")).toEqual(["'self'", "data:", "blob:"]);
  });

  it("allows no frame, no base, no plugin, and nothing else by default", () => {
    expect(directive(policy, "frame-ancestors")).toEqual(["'none'"]);
    expect(directive(policy, "base-uri")).toEqual(["'none'"]);
    expect(directive(policy, "object-src")).toEqual(["'none'"]);
    expect(directive(policy, "default-src")).toEqual(["'self'"]);
    expect(directive(policy, "font-src")).toEqual(["'self'", "data:"]);
  });

  it("names no issuer when none is configured: this origin only", () => {
    const bare = headerPolicy([]);
    expect(directive(bare, "connect-src")).toEqual(["'self'"]);
    expect(directive(bare, "form-action")).toEqual(["'self'"]);
  });

  it("drops anything in the configured list that is not an origin: a policy cannot be broken out of", () => {
    expect(
      connectSources(
        "https://ok.example ; script-src 'unsafe-inline' http://x.example/path * data: 'self'",
      ),
    ).toEqual(["https://ok.example"]);
    expect(connectSources(undefined)).toEqual([]);
    expect(connectSources("  ")).toEqual([]);
  });
});

describe("the meta half of the policy (each page's own)", () => {
  const sha = (text: string) => createHash("sha256").update(text, "utf8").digest("base64");

  it("allows the page's own files and exactly the inline scripts it carries, by their SHA-256", () => {
    const html =
      '<html><head><script>self.a=1</script><script src="/_next/static/x.js"></script></head>' +
      '<body><script id="d" type="application/json">{"no":"run"}</script><script>self.b=2</script></body></html>';
    const scripts = inlineScripts(html);
    expect(scripts).toEqual(["self.a=1", "self.b=2"]);
    const policy = metaPolicy(scripts.map(sha));
    expect(directive(policy, "script-src")).toEqual([
      "'self'",
      `'sha256-${sha("self.a=1")}'`,
      `'sha256-${sha("self.b=2")}'`,
    ]);
    expect(policy).not.toContain("unsafe-inline");
    expect(directive(policy, "object-src")).toEqual(["'none'"]);
    expect(directive(policy, "base-uri")).toEqual(["'none'"]);
  });

  it("hashes a script once however often it repeats, and names no connection (the desktop app connects elsewhere)", () => {
    const policy = metaPolicy(["abc", "abc"]);
    expect(directive(policy, "script-src")).toEqual(["'self'", "'sha256-abc'"]);
    expect(policy).not.toContain("connect-src");
  });

  it("is a meta tag whose content cannot end the attribute", () => {
    expect(metaTag("script-src 'self'")).toBe(
      `<meta http-equiv="Content-Security-Policy" content="script-src 'self'">`,
    );
    expect(metaTag('a"b')).toContain("a&quot;b");
  });
});
