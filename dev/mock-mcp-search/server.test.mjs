// Tests of server.mjs, with node's own runner: `node --test dev/mock-mcp-search/` (CI: workflow Compose, job
// `scripts`). They bind a free local port, so run them where binding is allowed.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { after, before, beforeEach, describe, it } from "node:test";
import { ICON_URI, PROTOCOL_VERSIONS, createMockServer, validateResults } from "./server.mjs";

const TOKEN = "test-token";
const results = JSON.parse(readFileSync(new URL("./results.json", import.meta.url), "utf8"));
const HEADERS = {
  "content-type": "application/json",
  accept: "application/json, text/event-stream",
  authorization: `Bearer ${TOKEN}`,
};

let server;
let base;

before(async () => {
  server = createMockServer({ token: TOKEN, results });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  base = `http://127.0.0.1:${server.address().port}`;
});
after(() => {
  server.closeAllConnections();
  server.close();
});
beforeEach(() => fetch(`${base}/__journal`, { method: "DELETE" }));

const rpc = (method, params, id = 1) => ({ jsonrpc: "2.0", id, method, ...(params === undefined ? {} : { params }) });
const post = (body, headers = HEADERS) =>
  fetch(`${base}/mcp`, { method: "POST", headers, body: typeof body === "string" ? body : JSON.stringify(body) });
const call = async (name, args) => (await (await post(rpc("tools/call", { name, arguments: args }))).json());
const textOf = (answer) => answer.result.content[0].text;

describe("the handshake", () => {
  it("answers initialize with the requested version, tools only, and no session", async () => {
    const res = await post(rpc("initialize", { protocolVersion: "2025-06-18", capabilities: {}, clientInfo: { name: "t", version: "1" } }));
    assert.equal(res.status, 200);
    assert.match(res.headers.get("content-type"), /^application\/json/);
    assert.equal(res.headers.get("mcp-session-id"), null);
    const { result } = await res.json();
    assert.equal(result.protocolVersion, "2025-06-18");
    assert.deepEqual(Object.keys(result.capabilities), ["tools"]);
    assert.equal(result.serverInfo.name, "mock-mcp-search");
    assert.equal(result.serverInfo.icons[0].src, ICON_URI);
  });

  it("answers a version it does not know with its newest", async () => {
    for (const asked of ["2026-07-28", "2024-11-05"]) {
      const { result } = await (await post(rpc("initialize", { protocolVersion: asked }))).json();
      assert.equal(result.protocolVersion, PROTOCOL_VERSIONS[0]);
    }
  });

  it("accepts a notification with 202 and no body", async () => {
    const res = await post({ jsonrpc: "2.0", method: "notifications/initialized" });
    assert.equal(res.status, 202);
    assert.equal(await res.text(), "");
  });

  it("answers ping", async () => {
    assert.deepEqual((await (await post(rpc("ping"))).json()).result, {});
  });

  it("answers a method it has not got, `server/discover` included, with -32601 so a modern client falls back", async () => {
    for (const method of ["server/discover", "resources/list"]) {
      const answer = await (await post(rpc(method))).json();
      assert.equal(answer.error.code, -32601);
      assert.equal(answer.id, 1);
    }
  });
});

describe("the tool", () => {
  it("lists web_search with an input schema and an icon that is a data: URI", async () => {
    const { result } = await (await post(rpc("tools/list"))).json();
    assert.equal(result.tools.length, 1);
    const [tool] = result.tools;
    assert.equal(tool.name, "web_search");
    assert.deepEqual(tool.inputSchema.required, ["query"]);
    assert.equal(tool.icons.length, 1);
    assert.match(tool.icons[0].src, /^data:image\/svg\+xml;base64,[A-Za-z0-9+/=]+$/);
    assert.equal(tool.icons[0].mimeType, "image/svg+xml");
    assert.ok(tool.icons[0].src.length < 2048);
    const svg = Buffer.from(tool.icons[0].src.split(",")[1], "base64").toString();
    assert.match(svg, /^<svg xmlns="http:\/\/www\.w3\.org\/2000\/svg"/);
  });

  it("answers the default list for a query without a keyword", async () => {
    const answer = await call("web_search", { query: "anything at all" });
    assert.equal(answer.result.isError, false);
    assert.equal(
      textOf(answer),
      [
        `1. ${results.default[0].title} — https://example.org/mock-search/1\n   ${results.default[0].snippet}`,
        `2. ${results.default[1].title} — https://example.org/mock-search/2\n   ${results.default[1].snippet}`,
      ].join("\n"),
    );
  });

  it("picks a list by keyword, ignoring case", async () => {
    const answer = await call("web_search", { query: "Who won the football WORLD CUP in 2014?" });
    assert.match(textOf(answer), /^1\. 2014 FIFA World Cup final \(mock\) — https:\/\/example\.org\/mock-search\/world-cup-2014\n {3}Germany won/);
    assert.match(textOf(answer), /\n2\. /);
  });

  it("answers `No results.` for [mock:empty] and a tool error for [mock:error]", async () => {
    const empty = await call("web_search", { query: "x [mock:empty]" });
    assert.deepEqual(empty.result, { content: [{ type: "text", text: "No results." }], isError: false });
    const failed = await call("web_search", { query: "[mock:error]" });
    assert.equal(failed.result.isError, true);
    assert.match(textOf(failed), /mock error/);
  });

  it("answers a tool execution error, not a protocol error, for a missing or empty query", async () => {
    for (const args of [{}, { query: "  " }, { query: 7 }, undefined, null, []]) {
      const answer = await call("web_search", args);
      assert.equal(answer.error, undefined, JSON.stringify(args));
      assert.equal(answer.result.isError, true, JSON.stringify(args));
    }
  });

  it("answers a protocol error for an unknown tool or bad params", async () => {
    assert.equal((await call("nope", {})).error.code, -32602);
    assert.equal((await (await post(rpc("tools/call", "x"))).json()).error.code, -32602);
  });

  it("journals the calls of the tool, in order, until DELETE", async () => {
    await call("web_search", { query: "first" });
    await call("web_search", { query: "second" });
    await call("nope", {});
    const { calls } = await (await fetch(`${base}/__journal`)).json();
    assert.deepEqual(calls.map((c) => [c.tool, c.arguments]), [["web_search", { query: "first" }], ["web_search", { query: "second" }]]);
    assert.ok(Number.isFinite(Date.parse(calls[0].at)));
    const emptied = await fetch(`${base}/__journal`, { method: "DELETE" });
    assert.equal(emptied.status, 200);
    assert.deepEqual((await (await fetch(`${base}/__journal`)).json()).calls, []);
  });
});

describe("the journal, as a client's headers reach it", () => {
  it("says whether the call carried the required token and keeps the X-* headers, lower-cased", async () => {
    const headers = { ...HEADERS, "X-Search-Tenant": "tenant-1", "X-Other": "two", "user-agent": "t" };
    await post(rpc("tools/call", { name: "web_search", arguments: { query: "x" } }), headers);
    const { calls } = await (await fetch(`${base}/__journal`)).json();
    assert.equal(calls.length, 1);
    assert.equal(calls[0].bearer, true);
    assert.deepEqual(calls[0].headers, { "x-search-tenant": "tenant-1", "x-other": "two" });
    await call("web_search", { query: "y" });
    assert.deepEqual((await (await fetch(`${base}/__journal`)).json()).calls[1].headers, {});
  });

  it("says bearer: false when the server requires no token", async () => {
    const open = createMockServer({ token: "", results });
    await new Promise((resolve) => open.listen(0, "127.0.0.1", resolve));
    try {
      const url = `http://127.0.0.1:${open.address().port}`;
      const { authorization: _, ...noAuth } = HEADERS;
      await fetch(`${url}/mcp`, { method: "POST", headers: noAuth, body: JSON.stringify(rpc("tools/call", { name: "web_search", arguments: { query: "z" } })) });
      assert.equal((await (await fetch(`${url}/__journal`)).json()).calls[0].bearer, false);
    } finally {
      open.closeAllConnections();
      open.close();
    }
  });
});

describe("the HTTP rules", () => {
  it("answers 401 without the token or with another, and journals nothing", async () => {
    const { authorization: _, ...open } = HEADERS;
    for (const headers of [open, { ...open, authorization: "Bearer wrong" }, { ...open, authorization: `Basic ${TOKEN}` }]) {
      const res = await post(rpc("tools/call", { name: "web_search", arguments: { query: "x" } }), headers);
      assert.equal(res.status, 401);
      assert.match(res.headers.get("www-authenticate"), /^Bearer/);
    }
    assert.deepEqual((await (await fetch(`${base}/__journal`)).json()).calls, []);
  });

  it("answers 405 to GET and DELETE on /mcp: no SSE stream, no sessions", async () => {
    for (const method of ["GET", "DELETE"]) {
      const res = await fetch(`${base}/mcp`, { method, headers: { ...HEADERS, accept: "text/event-stream" } });
      assert.equal(res.status, 405);
      assert.equal(res.headers.get("allow"), "POST");
    }
  });

  it("answers 415 for another content type and 406 when Accept lacks either type", async () => {
    assert.equal((await post(rpc("ping"), { ...HEADERS, "content-type": "text/plain" })).status, 415);
    assert.equal((await post(rpc("ping"), { ...HEADERS, accept: "application/json" })).status, 406);
    assert.equal((await post(rpc("ping"), { ...HEADERS, accept: "text/event-stream" })).status, 406);
    assert.equal((await post(rpc("ping"), { ...HEADERS, accept: "*/*" })).status, 200);
  });

  it("answers 400 for an unsupported MCP-Protocol-Version and accepts the supported ones", async () => {
    assert.equal((await post(rpc("ping"), { ...HEADERS, "mcp-protocol-version": "1999-01-01" })).status, 400);
    for (const version of PROTOCOL_VERSIONS) {
      assert.equal((await post(rpc("ping"), { ...HEADERS, "mcp-protocol-version": version })).status, 200);
    }
  });

  it("answers 403 to an Origin that is not the host", async () => {
    assert.equal((await post(rpc("ping"), { ...HEADERS, origin: "https://evil.example" })).status, 403);
    assert.equal((await post(rpc("ping"), { ...HEADERS, origin: base })).status, 200);
  });

  it("answers 400 to a body that is not one JSON-RPC object", async () => {
    const parse = await post("{not json");
    assert.equal(parse.status, 400);
    assert.equal((await parse.json()).error.code, -32700);
    for (const body of ["[]", JSON.stringify([rpc("ping")]), '{"jsonrpc":"1.0","id":1,"method":"ping"}', '{"jsonrpc":"2.0","id":null,"method":"ping"}']) {
      const res = await post(body);
      assert.equal(res.status, 400, body);
      assert.equal((await res.json()).error.code, -32600, body);
    }
  });

  it("answers 413 to a body over 64 KiB", async () => {
    const res = await post(JSON.stringify(rpc("ping", { pad: "x".repeat(70 * 1024) })));
    assert.equal(res.status, 413);
  });

  it("serves /healthz, 404 elsewhere, and keeps the journal and health open", async () => {
    const health = await fetch(`${base}/healthz`);
    assert.equal(health.status, 200);
    assert.deepEqual(await health.json(), { status: "ok" });
    assert.equal((await fetch(`${base}/nope`)).status, 404);
    assert.equal((await fetch(`${base}/__journal`, { method: "POST" })).status, 405);
  });
});

describe("without a token", () => {
  it("requires none", async () => {
    const open = createMockServer({ results });
    await new Promise((resolve) => open.listen(0, "127.0.0.1", resolve));
    try {
      const { authorization: _, ...headers } = HEADERS;
      const res = await fetch(`http://127.0.0.1:${open.address().port}/mcp`, { method: "POST", headers, body: JSON.stringify(rpc("ping")) });
      assert.equal(res.status, 200);
    } finally {
      open.closeAllConnections();
      open.close();
    }
  });
});

describe("results.json", () => {
  it("is valid, and a broken file is refused at startup", () => {
    assert.doesNotThrow(() => validateResults(results));
    assert.throws(() => validateResults({}), /default/);
    assert.throws(() => validateResults({ default: [{ title: "t", url: "u" }] }), /default/);
    assert.throws(() => validateResults({ default: results.default, keywords: { a: [] } }), /keyword "a"/);
  });
});
