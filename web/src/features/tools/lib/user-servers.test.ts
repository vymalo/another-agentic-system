import { describe, expect, it } from "vitest";
import type { ApiToolServer, ApiUserToolServer } from "@/lib/api/types";
import {
  attachable,
  buildChange,
  buildCreate,
  failureOf,
  hostOf,
  isOwn,
  moveWarning,
  nextAction,
  openableLink,
  scopesOf,
  standing,
  takesSecret,
} from "./user-servers";

const server = (over: Partial<ApiUserToolServer> = {}): ApiUserToolServer => ({
  id: "u-abcdefghij",
  name: "Search",
  own: true,
  status: "ready",
  url: "https://mcp.example.com/secret-path/mcp",
  auth: { kind: "bearer" },
  state: "connected",
  hasCredential: true,
  revision: "7",
  ...over,
});

describe("standing: what the person reads for a server's status and reason", () => {
  it("says ready in words, and what a ready server is for", () => {
    expect(standing(server())).toMatchObject({ label: "Ready", tone: "ok" });
  });

  it("asks for a key from a bearer or api_key server and for a sign-in from an oauth2 one, in the words of the reason", () => {
    const key = (reason: ApiUserToolServer["reason"]) =>
      standing(server({ status: "needs_auth", reason, state: "added", hasCredential: false }));
    const sign = (reason: ApiUserToolServer["reason"]) =>
      standing(
        server({
          status: "needs_auth",
          reason,
          auth: { kind: "oauth2" },
          state: "added",
          hasCredential: false,
        }),
      );
    expect(key("not_connected")).toMatchObject({ label: "Needs a key", tone: "action" });
    expect(key("reauthorize").hint).toMatch(/key was refused|ended/i);
    expect(key("disconnected").hint).toMatch(/disconnected/i);
    expect(sign("not_connected")).toMatchObject({ label: "Needs a sign-in", tone: "action" });
    expect(sign("reauthorize").hint).toMatch(/sign-in ended/i);
    expect(sign("disconnected").hint).toMatch(/disconnected/i);
  });

  it("says why a server is unavailable, and what to do about it", () => {
    const why = (reason: ApiUserToolServer["reason"]) =>
      standing(server({ status: "unavailable", reason }));
    expect(why("not_permitted")).toMatchObject({ label: "Not available", tone: "blocked" });
    expect(why("not_permitted").hint).toMatch(/roles/);
    expect(why("not_permitted").hint).toMatch(/delete/i);
    expect(why("egress_refused").hint).toMatch(/address/);
    expect(why("broker_down").hint).toMatch(/credentials/);
  });

  it("has words for a status or a reason it does not know, and never an empty line", () => {
    const odd = standing({
      ...server(),
      status: "paused" as unknown as ApiUserToolServer["status"],
    });
    expect(odd.label.length).toBeGreaterThan(0);
    expect(odd.hint.length).toBeGreaterThan(0);
    const unknownReason = standing(
      server({ status: "unavailable", reason: "cosmic_rays" as unknown as "broker_down" }),
    );
    expect(unknownReason.hint.length).toBeGreaterThan(0);
  });
});

describe("nextAction: the one thing that moves a server that needs the person", () => {
  it("is Connect for oauth2 and a new key for bearer and api_key, only while the server needs one", () => {
    const needs = (auth: ApiUserToolServer["auth"], status: ApiUserToolServer["status"]) =>
      nextAction(server({ auth, status }));
    expect(needs({ kind: "oauth2" }, "needs_auth")).toBe("connect");
    expect(needs({ kind: "bearer" }, "needs_auth")).toBe("key");
    expect(needs({ kind: "api_key", header: "X-Key" }, "needs_auth")).toBe("key");
    expect(needs({ kind: "none" }, "needs_auth")).toBeNull();
    expect(needs({ kind: "oauth2" }, "ready")).toBeNull();
    expect(needs({ kind: "oauth2" }, "unavailable")).toBeNull();
  });
});

describe("hostOf and scopesOf", () => {
  it("shows the host of an address and never its path, which can hold a secret", () => {
    expect(hostOf("https://mcp.example.com/secret-path/mcp")).toBe("mcp.example.com");
    expect(hostOf("not a url")).toBe("not a url");
  });

  it("reads scopes as words separated by spaces or commas, once each", () => {
    expect(scopesOf(" read, write  read\nadmin ")).toEqual(["read", "write", "admin"]);
    expect(scopesOf("")).toEqual([]);
  });
});

describe("takesSecret", () => {
  it("is true for the two kinds that are given a token or a key", () => {
    expect(takesSecret("bearer")).toBe(true);
    expect(takesSecret("api_key")).toBe(true);
    expect(takesSecret("none")).toBe(false);
    expect(takesSecret("oauth2")).toBe(false);
  });
});

describe("buildCreate: what is sent to add a server", () => {
  const form = {
    name: "  Search  ",
    url: " https://mcp.example.com/mcp ",
    kind: "bearer" as const,
    header: "",
    scopes: "",
    clientId: "",
    secret: "tok-123456789",
  };

  it("sends the trimmed name and address, the kind, and the secret only for a kind that takes one", () => {
    expect(buildCreate(form)).toEqual({
      name: "Search",
      url: "https://mcp.example.com/mcp",
      auth: { kind: "bearer" },
      secret: "tok-123456789",
    });
    expect(buildCreate({ ...form, kind: "none" })).toEqual({
      name: "Search",
      url: "https://mcp.example.com/mcp",
      auth: { kind: "none" },
    });
  });

  it("names the header of an api_key, and the scopes and client of an oauth2 server when given", () => {
    expect(buildCreate({ ...form, kind: "api_key", header: " X-Api-Key " }).auth).toEqual({
      kind: "api_key",
      header: "X-Api-Key",
    });
    const oauth = buildCreate({ ...form, kind: "oauth2", scopes: "read write", clientId: " c-1 " });
    expect(oauth.auth).toEqual({ kind: "oauth2", scopes: ["read", "write"], clientId: "c-1" });
    expect(oauth).not.toHaveProperty("secret");
    expect(buildCreate({ ...form, kind: "oauth2" }).auth).toEqual({ kind: "oauth2" });
  });
});

describe("buildChange: an edit sends only what differs, with the revision it was based on", () => {
  const was = server();
  const same = {
    name: was.name,
    url: was.url,
    kind: "bearer" as const,
    header: "",
    scopes: "",
    clientId: "",
    secret: "",
  };

  it("is nothing when nothing differs", () => {
    expect(buildChange(was, same)).toBeNull();
  });

  it("renames with the revision, and keeps the address and the authentication out of it", () => {
    expect(buildChange(was, { ...same, name: " Renamed " })).toEqual({
      name: "Renamed",
      revision: "7",
    });
  });

  it("re-keys with the secret and the revision only", () => {
    expect(buildChange(was, { ...same, secret: "new-token-123" })).toEqual({
      secret: "new-token-123",
      revision: "7",
    });
  });

  it("moves with the new address and, when the kind or its settings changed, the whole auth object", () => {
    expect(buildChange(was, { ...same, url: "https://other.example.com/mcp" })).toEqual({
      url: "https://other.example.com/mcp",
      revision: "7",
    });
    expect(
      buildChange(was, { ...same, kind: "api_key", header: "X-Key", secret: "key-12345678" }),
    ).toEqual({
      auth: { kind: "api_key", header: "X-Key" },
      secret: "key-12345678",
      revision: "7",
    });
    const flow = server({ auth: { kind: "oauth2", scopes: ["read"] } });
    expect(
      buildChange(flow, {
        ...same,
        kind: "oauth2",
        scopes: "read write",
        url: flow.url,
        name: flow.name,
      }),
    ).toEqual({ auth: { kind: "oauth2", scopes: ["read", "write"] }, revision: "7" });
    // the same scopes in another order are the same set: nothing differs
    expect(
      buildChange(flow, {
        ...same,
        kind: "oauth2",
        scopes: "read",
        url: flow.url,
        name: flow.name,
      }),
    ).toBeNull();
  });
});

describe("moveWarning: what a change forgets", () => {
  it("warns when the address or the kind changes on a server that holds a credential, and not for a rename", () => {
    const was = server();
    const form = {
      name: was.name,
      url: was.url,
      kind: "bearer" as const,
      header: "",
      scopes: "",
      clientId: "",
      secret: "",
    };
    expect(moveWarning(was, form)).toBeNull();
    expect(moveWarning(was, { ...form, name: "Other" })).toBeNull();
    expect(moveWarning(was, { ...form, url: "https://other.example.com/mcp" })).toMatch(
      /forget|asked/i,
    );
    expect(moveWarning(was, { ...form, kind: "oauth2" })).toMatch(/forget|asked/i);
    // nothing is kept for a server that has no credential: nothing to forget
    expect(
      moveWarning(server({ hasCredential: false }), {
        ...form,
        url: "https://other.example.com/mcp",
      }),
    ).toBeNull();
  });
});

describe("failureOf: the words come from the status and the code, never a guessed message", () => {
  const f = (status: number, code?: string, detail?: string, retryAfter: string | null = null) =>
    failureOf({ status, code, detail }, { retryAfter, limit: 8 });

  it("says a stale revision as what to do, and reads the list again", () => {
    const stale = f(409, "stale_revision");
    expect(stale.message).toMatch(/changed since you opened/i);
    expect(stale.reread).toBe(true);
  });

  it("says the limit with the number the list gave", () => {
    expect(f(409, "too_many_servers").message).toContain("8");
    // the number is the list's, not ours
    expect(
      failureOf({ status: 409, code: "too_many_servers" }, { retryAfter: null, limit: 5 }).message,
    ).toContain("5 servers");
  });

  it("names the field a code is about, so the screen can tie the words to it", () => {
    expect(f(400, "url_refused", "x").field).toBe("url");
    expect(f(422, "credentials_not_kept").field).toBe("kind");
    expect(f(409, "stale_revision").field).toBeUndefined();
    expect(f(400, undefined, "`secret` is short").field).toBeUndefined();
  });

  it("says a refused address with the guard's own sentence, never an address of ours", () => {
    const refused = f(
      400,
      "url_refused",
      "this deployment does not allow connections to that address",
    );
    expect(refused.message).toMatch(/does not allow connections/);
    // a refused sign-in address (authorize) is a 422 with another code and its own words
    expect(f(422, "egress_refused", "the guard's sentence").message).toMatch(/sign-in/i);
  });

  it("says each 422 and 503 by its code", () => {
    expect(f(422, "credentials_not_kept").message).toMatch(/keeps no credentials/);
    expect(f(422, "oauth_not_configured").message).toMatch(/ask whoever runs/i);
    expect(f(503, "broker_down").message).toMatch(/credential/);
    expect(f(503, "authorization_unavailable").message).toMatch(/sign-in could not be started/i);
    expect(f(503).message).toMatch(/another request/);
    expect(f(503).reread).toBe(true);
  });

  it("says the wait a rate limit asks for", () => {
    expect(f(429, "rate_limited", undefined, "6").message).toMatch(/6 seconds/);
    expect(f(429, "rate_limited").message).toMatch(/try again in a moment/i);
  });

  it("says a missing permission as roles, a 404 as a server that is gone and an unknown failure as unknown", () => {
    expect(f(403, "forbidden").message).toMatch(/roles do not let you/i);
    expect(f(403, "no_access").message).toMatch(/no access/i);
    expect(f(404).message).toMatch(/no longer exists/i);
    expect(f(404).reread).toBe(true);
    const odd = f(500);
    expect(odd.message).toMatch(/may have been made/i);
    expect(odd.reread).toBe(true);
  });

  it("shows a 400's own words (the server never quotes a value in them)", () => {
    expect(f(400, undefined, "`secret` is 8 to 4096 printable characters").message).toContain(
      "8 to 4096",
    );
    expect(f(400).message.length).toBeGreaterThan(0);
  });
});

describe("attachable and isOwn: what the picker offers", () => {
  const listed = (over: Partial<ApiToolServer>): ApiToolServer => ({ id: "x", name: "X", ...over });
  it("offers a deployment's server and a person's own server only while it is ready", () => {
    expect(attachable(listed({}))).toBe(true);
    expect(attachable(listed({ own: true, status: "ready" }))).toBe(true);
    expect(attachable(listed({ own: true, status: "needs_auth", reason: "not_connected" }))).toBe(
      false,
    );
    expect(attachable(listed({ own: true, status: "unavailable", reason: "broker_down" }))).toBe(
      false,
    );
  });
  it("marks a person's own", () => {
    expect(isOwn(listed({ own: true, status: "ready" }))).toBe(true);
    expect(isOwn(listed({}))).toBe(false);
  });
  it("is the moment to say what a server needs", () => {
    expect(
      standing({ status: "needs_auth", reason: "not_connected", auth: { kind: "oauth2" } }).label,
    ).toBe("Needs a sign-in");
  });
});

describe("openableLink: the one rule for a sign-in link before anything is opened", () => {
  const issuer = "https://login.stranger.example";

  it("opens an https link and says where it goes: the host of the link itself", () => {
    expect(openableLink("https://login.stranger.example/authorize?state=s", issuer)).toEqual({
      ok: true,
      href: "https://login.stranger.example/authorize?state=s",
      host: "login.stranger.example",
      issuerHost: "login.stranger.example",
      sameHost: true,
    });
  });

  it("opens a link on another host than the issuer (RFC 8414 lets an authorization endpoint live elsewhere), and says both hosts", () => {
    const link = openableLink("https://accounts.other.example/o/authorize?x=1", issuer);
    expect(link).toMatchObject({
      ok: true,
      host: "accounts.other.example",
      issuerHost: "login.stranger.example",
      sameHost: false,
    });
  });

  it("opens http only on this machine, for a dev authorization server", () => {
    for (const url of ["http://127.0.0.1:4010/a", "http://localhost/a", "http://[::1]:9/a"]) {
      expect(openableLink(url, issuer).ok, url).toBe(true);
    }
    expect(openableLink("http://example.com/a", issuer).ok).toBe(false);
  });

  it("opens nothing else: javascript:, data:, blob:, file:, a relative or malformed link, user information, nothing", () => {
    for (const url of [
      "javascript:alert(1)",
      "JaVaScRiPt:alert(1)",
      "data:text/html,<script>1</script>",
      "blob:https://login.stranger.example/x",
      "file:///etc/passwd",
      "/authorize?state=s",
      "//login.stranger.example/authorize",
      "not a url",
      "https://user:pass@login.stranger.example/a",
      "",
    ]) {
      expect(openableLink(url, issuer), url).toEqual({ ok: false });
    }
  });
});
