import { describe, expect, it } from "vitest";
import { connectSources, contentSecurityPolicy, makeNonce } from "./csp";

const directive = (policy: string, name: string): string[] =>
  (policy.split("; ").find((d) => d.startsWith(`${name} `)) ?? "").split(" ").slice(1);

describe("the content security policy", () => {
  const policy = contentSecurityPolicy({
    nonce: "abc123",
    connect: "https://id.example https://*.id.example:8443",
  });

  it("allows only our own scripts: self, this request's nonce and strict-dynamic, never eval or inline", () => {
    expect(directive(policy, "script-src")).toEqual([
      "'self'",
      "'nonce-abc123'",
      "'strict-dynamic'",
    ]);
  });

  it("lets the page connect to itself and to the issuer", () => {
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
    const bare = contentSecurityPolicy({ nonce: "n" });
    expect(directive(bare, "connect-src")).toEqual(["'self'"]);
    expect(directive(bare, "form-action")).toEqual(["'self'"]);
  });

  it("relaxes only what next dev needs: eval and a socket", () => {
    const dev = contentSecurityPolicy({ nonce: "n", dev: true });
    expect(directive(dev, "script-src")).toContain("'unsafe-eval'");
    expect(directive(dev, "connect-src")).toContain("ws:");
    expect(policy).not.toContain("unsafe-eval");
    expect(policy).not.toContain("ws:");
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

  it("makes a new nonce every time, base64 without anything a header could not hold", () => {
    const a = makeNonce();
    const b = makeNonce();
    expect(a).not.toBe(b);
    expect(a).toMatch(/^[A-Za-z0-9+/]{22}==$/);
  });
});
