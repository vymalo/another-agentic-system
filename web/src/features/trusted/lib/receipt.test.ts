import { describe, expect, it } from "vitest";
import { componentNames, OWN_CATALOG } from "@/features/chat/lib/a2ui/catalog";
import { VOCABULARY } from "@/features/chat/lib/a2ui/limits";
import { prepareSurface } from "@/features/chat/lib/a2ui/prepare";
import type { ApiUserToolServer } from "@/lib/api/types";
import { isDrawn, keyTakers, receiptOf, rekeyBody, shownName } from "./receipt";

/*
 * The trusted component's pure half (ADR 0083): the components this build draws, the receipt that reaches the agent,
 * and the body of a re-key. The invariant the tests keep is the ADR's: **the receipt holds no value of the secret**, and
 * the re-key sends nothing that could move a server.
 */

const server = (over: Partial<ApiUserToolServer> = {}): ApiUserToolServer => ({
  id: "u-abcdefghij",
  name: "My search",
  own: true,
  status: "ready",
  url: "https://mcp.example.com/mcp",
  auth: { kind: "bearer" },
  state: "connected",
  hasCredential: true,
  revision: "7",
  ...over,
});

describe("the components this build draws", () => {
  it("draws McpCredential and nothing else, so every other name is words and a link", () => {
    expect(isDrawn("McpCredential")).toBe(true);
    for (const name of ["ApiKeyCreate", "ModelServer", "SecretGenerate", "SecretCollect"]) {
      expect(isDrawn(name)).toBe(false);
    }
    // matched exactly, as the contract matches it
    expect(isDrawn("mcpcredential")).toBe(false);
    expect(isDrawn("McpCredential ")).toBe(false);
  });

  it("is in no catalog, so a surface that names one is refused and never drawn as this card", () => {
    expect(componentNames(OWN_CATALOG.catalog)).not.toContain("McpCredential");
    expect(VOCABULARY as readonly string[]).not.toContain("McpCredential");
    // the existing rule, proved: a surface of this app's catalog that names it is refused whole
    const prepared = prepareSurface([
      {
        version: "1.0",
        createSurface: { surfaceId: "s1", catalogId: OWN_CATALOG.catalog.catalogId },
      },
      {
        version: "1.0",
        updateComponents: {
          surfaceId: "s1",
          components: [{ id: "root", component: "McpCredential" }],
        },
      },
    ]);
    expect(prepared.kind).toBe("refused");
  });
});

describe("the receipt", () => {
  it("names the server and the scopes the person kept, and nothing else", () => {
    expect(receiptOf("created", server(), ["search.read"])).toEqual({
      status: "created",
      id: "u-abcdefghij",
      name: "My search",
      scopes: ["search.read"],
    });
    expect(receiptOf("updated", server({ name: "Wiki" }), [])).toEqual({
      status: "updated",
      id: "u-abcdefghij",
      name: "Wiki",
    });
  });

  it("keeps each scope once, and never a value of the secret", () => {
    const secret = "tok-9f3a-this-is-a-secret-value";
    const receipt = receiptOf("created", server(), ["search.read", "search.read", "search.write"]);
    expect(receipt.scopes).toEqual(["search.read", "search.write"]);
    expect(JSON.stringify(receipt)).not.toContain(secret);
    expect(JSON.stringify(receipt)).not.toContain("tok-");
    // the members are the contract's, and `last4` is absent: no route returns a suffix
    expect(Object.keys(receipt).sort()).toEqual(["id", "name", "scopes", "status"]);
  });
});

describe("a new key for a server the person has", () => {
  it("sends the secret and the revision, and nothing that could move it", () => {
    expect(rekeyBody(server(), "tok-new-value")).toEqual({
      secret: "tok-new-value",
      revision: "7",
    });
    expect(Object.keys(rekeyBody(server(), "x")).sort()).toEqual(["revision", "secret"]);
  });

  it("offers only the servers a key can be given to", () => {
    const servers = [
      server({ id: "u-aaaaaaaaaa", auth: { kind: "bearer" } }),
      server({ id: "u-bbbbbbbbbb", auth: { kind: "api_key", header: "X-Api-Key" } }),
      server({ id: "u-cccccccccc", auth: { kind: "oauth2" } }),
      server({ id: "u-dddddddddd", auth: { kind: "none" } }),
    ];
    expect(keyTakers(servers).map((s) => s.id)).toEqual(["u-aaaaaaaaaa", "u-bbbbbbbbbb"]);
  });
});

describe("a name from an agent", () => {
  it("is one short line: control characters become ?, and it is cut", () => {
    expect(shownName("Mcp\nCredential")).toBe("Mcp?Credential");
    expect(shownName("x".repeat(80))).toHaveLength(40);
  });
});
