// Tests of server.mjs, with node's own runner: `node --test dev/searxng-mcp/server.test.mjs` (CI: workflow Compose, job `scripts`). They bind
// free local ports, so run them where binding is allowed. Nothing leaves the machine: SearXNG and the pages are fakes here.
import assert from "node:assert/strict";
import { createServer } from "node:http";
import { after, before, describe, it } from "node:test";
import {
  FETCH_TOOL,
  ICON_URI,
  MAX_RESULTS,
  MAX_TEXT_BYTES,
  PROTOCOL_VERSIONS,
  SEARCH_TOOL,
  createProvider,
  createSearchServer,
  cutUtf8,
  fetchPage,
  formatResults,
  guardedLookup,
  htmlToText,
  isBlockedAddress,
} from "./server.mjs";

const listen = (server) =>
  new Promise((resolve) => server.listen(0, "127.0.0.1", () => resolve(`http://127.0.0.1:${server.address().port}`)));
const close = (server) => {
  server.closeAllConnections();
  server.close();
};

// What the tests allow on top of the rule: the loopback address, where the fakes listen. Every other address stays refused.
const loopbackToo = (address) => address !== "127.0.0.1" && isBlockedAddress(address);

// ---------------------------------------------------------------------------------------------- the fakes

const requests = { searxng: [], brave: [], tavily: [] };
const searxngResults = Array.from({ length: 14 }, (_, i) => ({
  title: `Result ${i + 1}`,
  url: `https://example.org/${i + 1}`,
  content: i === 0 ? `${"long ".repeat(200)}end` : `Snippet ${i + 1}`,
}));
searxngResults.splice(1, 0, { title: "A script link", url: "javascript:alert(1)", content: "dropped" });

let fakeSearch;
let searchBase;
before(async () => {
  fakeSearch = createServer((req, res) => {
    const url = new URL(req.url, "http://x");
    const send = (status, body) => {
      res.writeHead(status, { "content-type": "application/json" });
      res.end(typeof body === "string" ? body : JSON.stringify(body));
    };
    if (url.pathname === "/search") {
      requests.searxng.push(Object.fromEntries(url.searchParams));
      const q = url.searchParams.get("q");
      if (q === "boom") return send(500, { error: "x" });
      if (q === "html") return send(200, "<html>not json</html>");
      if (q === "nothing") return send(200, { results: [] });
      return send(200, { results: searxngResults });
    }
    if (url.pathname === "/brave") {
      requests.brave.push({ ...Object.fromEntries(url.searchParams), token: req.headers["x-subscription-token"] });
      return send(200, { web: { results: [{ title: "B", url: "https://brave.example/1", description: "from brave" }] } });
    }
    if (url.pathname === "/tavily" && req.method === "POST") {
      let body = "";
      req.on("data", (c) => (body += c));
      req.on("end", () => {
        requests.tavily.push({ ...JSON.parse(body), authorization: req.headers.authorization });
        send(200, { results: [{ title: "T", url: "https://tavily.example/1", content: "from tavily" }] });
      });
      return;
    }
    return send(404, {});
  });
  searchBase = await listen(fakeSearch);
});
after(() => close(fakeSearch));

const PAGE = `<!doctype html><html><head><title>Fake &amp; page</title><style>.x{color:red}</style>
<script>var secret = "SCRIPT-BODY";</script></head><body><!-- a comment --><h1>Heading</h1>
<p>First <b>paragraph</b> &mdash; with &lt;entities&gt; &#169; &#x41;.</p><ul><li>one</li><li>two</li></ul>
<noscript>NOSCRIPT-BODY</noscript><script>unclosed("UNCLOSED-SCRIPT")`;

let pages;
let pageBase;
before(async () => {
  pages = createServer((req, res) => {
    const path = new URL(req.url, "http://x").pathname;
    const send = (status, type, body, headers = {}) => {
      res.writeHead(status, { "content-type": type, ...headers });
      res.end(body);
    };
    if (path === "/page") return send(200, "text/html; charset=utf-8", PAGE);
    if (path === "/plain") return send(200, "text/plain", "just text\r\nsecond line");
    if (path === "/big") return send(200, "text/plain", "é".repeat(100_000)); // 200 000 bytes
    if (path === "/huge") return send(200, "text/plain", "x".repeat(3 * 1024 * 1024));
    if (path === "/image") return send(200, "image/png", "PNG");
    if (path === "/missing") return send(404, "text/plain", "no");
    if (path === "/to-page") return send(302, "text/plain", "", { location: "/page" });
    if (path === "/to-metadata") return send(302, "text/plain", "", { location: "http://169.254.169.254/latest/meta-data/" });
    if (path === "/to-private-name") return send(302, "text/plain", "", { location: "http://internal.test/secret" });
    if (path === "/loop") return send(302, "text/plain", "", { location: "/loop" });
    if (path === "/slow") return; // never answers
    return send(404, "text/plain", "not found");
  });
  pageBase = await listen(pages);
});
after(() => close(pages));

// A name resolver for the tests: `internal.test` is a private address, `public.test` the loopback the fake pages listen on.
const fakeResolve = (hostname, _options, callback) => {
  const table = { "internal.test": "10.0.0.5", "mixed.test": "127.0.0.1", "v6.test": "fd00::1" };
  if (hostname === "mixed.test") return callback(null, [{ address: "93.184.216.34", family: 4 }, { address: "10.1.1.1", family: 4 }]);
  if (!table[hostname]) return callback(Object.assign(new Error("ENOTFOUND"), { code: "ENOTFOUND" }));
  return callback(null, [{ address: table[hostname], family: table[hostname].includes(":") ? 6 : 4 }]);
};
const testLookup = guardedLookup({ resolve: fakeResolve, isBlocked: loopbackToo });

// ---------------------------------------------------------------------------------------------- addresses

describe("isBlockedAddress", () => {
  it("refuses loopback, private, link-local, shared, multicast and reserved addresses", () => {
    for (const a of [
      "127.0.0.1", "127.255.255.254", "10.0.0.1", "10.255.255.255", "172.16.0.1", "172.31.255.255", "192.168.1.1",
      "169.254.169.254", "169.254.0.1", "100.64.0.1", "0.0.0.0", "224.0.0.1", "255.255.255.255", "198.18.0.1",
      "::1", "::", "fe80::1", "fd00::1", "fc00::1", "ff02::1", "::ffff:10.0.0.1", "::ffff:127.0.0.1", "::ffff:a9fe:a9fe",
      "64:ff9b::a00:1", "2002:7f00:1::", "2001:db8::1",
    ]) {
      assert.equal(isBlockedAddress(a), true, a);
    }
  });

  it("allows public addresses", () => {
    for (const a of ["8.8.8.8", "93.184.216.34", "172.15.0.1", "172.32.0.1", "11.0.0.1", "2606:4700:4700::1111", "::ffff:8.8.8.8"]) {
      assert.equal(isBlockedAddress(a), false, a);
    }
  });

  it("refuses what is not an address", () => {
    assert.equal(isBlockedAddress("example.org"), true);
    assert.equal(isBlockedAddress(""), true);
  });
});

describe("fetchPage refuses private destinations (SSRF)", () => {
  const refused = async (url, options) => {
    const answer = await fetchPage(url, options);
    assert.equal(answer.ok, false, url);
    return answer.error;
  };

  it("an address in the link: loopback, private, link-local, IPv6", async () => {
    for (const url of [
      "http://127.0.0.1:1/", "http://10.1.2.3/", "http://192.168.0.10:8080/x", "http://169.254.169.254/latest/meta-data/",
      "http://[::1]:1/", "http://[fd00::1]/", "http://[::ffff:10.0.0.1]/", "http://0.0.0.0/", "http://2130706433/", "http://0x7f.1/",
    ]) {
      assert.match(await refused(url), /Refused|private or reserved/, url);
    }
  });

  it("a name that resolves to one, also when it is only one of its addresses", async () => {
    for (const host of ["internal.test", "mixed.test", "v6.test"]) {
      assert.match(await refused(`http://${host}/`, { lookup: guardedLookup({ resolve: fakeResolve }) }), /Refused: .*private or reserved/, host);
    }
  });

  it("localhost, with the real resolver", async () => {
    assert.match(await refused("http://localhost:1/"), /Refused/);
  });

  it("a redirect to a private address, by address and by name", async () => {
    assert.match(await refused(`${pageBase}/to-metadata`, { isBlocked: loopbackToo }), /169\.254\.169\.254/);
    assert.match(await refused(`${pageBase}/to-private-name`, { isBlocked: loopbackToo, lookup: testLookup }), /internal\.test/);
  });

  it("other schemes, credentials in the link, and a redirect loop", async () => {
    assert.match(await refused("file:///etc/passwd"), /Only http and https/);
    assert.match(await refused("ftp://example.org/x"), /Only http and https/);
    assert.match(await refused("http://user:pw@example.org/"), /user name or password/);
    assert.match(await refused("not a url"), /not a valid URL/);
    assert.match(await refused(`${pageBase}/loop`, { isBlocked: loopbackToo }), /redirects/);
  });

  it("the loopback fake is itself refused by the default rule (the carve-out is the tests' only)", async () => {
    assert.match(await refused(`${pageBase}/page`), /Refused/);
  });
});

describe("fetchPage reads text", () => {
  const options = { isBlocked: loopbackToo };

  it("turns HTML into plain text: no script, style, comment or noscript, entities decoded, the title apart", async () => {
    const page = await fetchPage(`${pageBase}/page`, options);
    assert.equal(page.ok, true);
    assert.equal(page.title, "Fake & page");
    assert.match(page.text, /^Heading\n/);
    assert.match(page.text, /First paragraph — with <entities> © A\./);
    assert.match(page.text, /\n- one\n- two/);
    for (const gone of ["SCRIPT-BODY", "NOSCRIPT-BODY", "UNCLOSED-SCRIPT", "color:red", "a comment", "<b>", "<p>"]) {
      assert.equal(page.text.includes(gone), false, gone);
    }
  });

  it("follows a redirect to a public page", async () => {
    const page = await fetchPage(`${pageBase}/to-page`, options);
    assert.equal(page.ok, true);
    assert.equal(page.url, `${pageBase}/page`);
  });

  it("returns plain text as it is", async () => {
    const page = await fetchPage(`${pageBase}/plain`, options);
    assert.equal(page.text, "just text\nsecond line");
    assert.equal(page.cut, false);
  });

  it("cuts the text at 64 KiB, in whole characters", async () => {
    const page = await fetchPage(`${pageBase}/big`, options);
    assert.equal(page.ok, true);
    assert.equal(page.cut, true);
    assert.ok(Buffer.byteLength(page.text) <= MAX_TEXT_BYTES);
    assert.ok(Buffer.byteLength(page.text) >= MAX_TEXT_BYTES - 1);
    assert.equal(page.text.includes("�"), false);
    assert.equal(MAX_TEXT_BYTES, 65536);
  });

  it("reads at most 2 MiB of a huge page and still answers", async () => {
    const page = await fetchPage(`${pageBase}/huge`, options);
    assert.equal(page.ok, true);
    assert.equal(page.cut, true);
    assert.equal(Buffer.byteLength(page.text), MAX_TEXT_BYTES);
  });

  it("refuses a page that is not text, and a status of 400 or more", async () => {
    assert.match((await fetchPage(`${pageBase}/image`, options)).error, /Not a text page \(image\/png\)/);
    assert.match((await fetchPage(`${pageBase}/missing`, options)).error, /HTTP 404/);
  });

  it("gives up after its timeout", async () => {
    const started = Date.now();
    const answer = await fetchPage(`${pageBase}/slow`, { ...options, timeoutMs: 300 });
    assert.equal(answer.ok, false);
    assert.match(answer.error, /Timed out/);
    assert.ok(Date.now() - started < 3000);
  });
});

describe("text helpers", () => {
  it("cutUtf8 never splits a character", () => {
    assert.deepEqual(cutUtf8("abc", 10), { text: "abc", cut: false });
    assert.deepEqual(cutUtf8("aé", 2), { text: "a", cut: true }); // é is two bytes: the second would be half of it
    assert.deepEqual(cutUtf8("日本語", 4), { text: "日", cut: true });
  });

  it("htmlToText survives broken markup", () => {
    assert.equal(htmlToText("<p>a<b>b</p>< c").text, "ab\n< c");
    assert.equal(htmlToText("").text, "");
  });
});

// ---------------------------------------------------------------------------------------------- the providers

describe("web_search results", () => {
  it("formats at most `limit` entries, with the title and snippet cut and unsafe links dropped", () => {
    const text = formatResults(searxngResults.map((r) => ({ title: r.title, url: r.url, snippet: r.content })), 3);
    const lines = text.split("\n");
    assert.equal(lines.length, 6);
    assert.match(lines[0], /^1\. Result 1 — https:\/\/example\.org\/1$/);
    assert.ok(lines[1].length <= 3 + 300, `snippet is ${lines[1].length} long`);
    assert.match(lines[1], /…$/);
    assert.match(lines[2], /^2\. Result 2 /); // the javascript: link was skipped
    assert.equal(formatResults([], 5), "No results.");
  });

  it("asks SearXNG for the JSON format", async () => {
    const provider = createProvider({ SEARXNG_URL: searchBase });
    requests.searxng.length = 0;
    const raw = await provider.search("rust language", 5);
    assert.deepEqual(requests.searxng, [{ q: "rust language", format: "json" }]);
    assert.equal(raw.length, searxngResults.length);
  });

  it("Brave: the key in its header, the count asked for", async () => {
    const provider = createProvider({ WEB_SEARCH_PROVIDER: "brave", BRAVE_API_KEY: "k-brave", BRAVE_API_URL: `${searchBase}/brave` });
    assert.deepEqual(await provider.search("q", 4), [{ title: "B", url: "https://brave.example/1", snippet: "from brave" }]);
    assert.deepEqual(requests.brave.at(-1), { q: "q", count: "4", token: "k-brave" });
  });

  it("Tavily: the key as a bearer token, the count asked for", async () => {
    const provider = createProvider({ WEB_SEARCH_PROVIDER: "tavily", TAVILY_API_KEY: "k-tav", TAVILY_API_URL: `${searchBase}/tavily` });
    assert.deepEqual(await provider.search("q", 3), [{ title: "T", url: "https://tavily.example/1", snippet: "from tavily" }]);
    assert.deepEqual(requests.tavily.at(-1), { query: "q", max_results: 3, authorization: "Bearer k-tav" });
  });

  it("refuses a configuration that cannot work", () => {
    assert.throws(() => createProvider({}), /SEARXNG_URL/);
    assert.throws(() => createProvider({ WEB_SEARCH_PROVIDER: "brave" }), /BRAVE_API_KEY/);
    assert.throws(() => createProvider({ WEB_SEARCH_PROVIDER: "tavily" }), /TAVILY_API_KEY/);
    assert.throws(() => createProvider({ WEB_SEARCH_PROVIDER: "bing" }), /searxng, brave or tavily/);
  });
});

// ---------------------------------------------------------------------------------------------- the MCP surface

describe("the MCP endpoint", () => {
  const TOKEN = "test-token";
  const HEADERS = { "content-type": "application/json", accept: "application/json, text/event-stream", authorization: `Bearer ${TOKEN}` };
  let server;
  let base;
  before(async () => {
    server = createSearchServer({
      token: TOKEN,
      provider: createProvider({ SEARXNG_URL: searchBase }),
      fetchOptions: { isBlocked: loopbackToo },
    });
    base = await listen(server);
  });
  after(() => close(server));

  const rpc = (method, params, id = 1) => ({ jsonrpc: "2.0", id, method, ...(params === undefined ? {} : { params }) });
  const post = (body, headers = HEADERS) => fetch(`${base}/mcp`, { method: "POST", headers, body: typeof body === "string" ? body : JSON.stringify(body) });
  const call = async (name, args) => (await (await post(rpc("tools/call", { name, arguments: args }))).json()).result;

  it("answers initialize with the requested version, tools only, and no session", async () => {
    const res = await post(rpc("initialize", { protocolVersion: "2025-06-18", capabilities: {}, clientInfo: { name: "t", version: "1" } }));
    assert.equal(res.status, 200);
    assert.equal(res.headers.get("mcp-session-id"), null);
    const { result } = await res.json();
    assert.equal(result.protocolVersion, "2025-06-18");
    assert.deepEqual(Object.keys(result.capabilities), ["tools"]);
    assert.equal(result.serverInfo.name, "searxng-mcp");
    const newest = await (await post(rpc("initialize", { protocolVersion: "2024-11-05" }))).json();
    assert.equal(newest.result.protocolVersion, PROTOCOL_VERSIONS[0]);
  });

  it("lists web_search (titled `Web search`) and fetch, with their schemas", async () => {
    const { result } = await (await post(rpc("tools/list"))).json();
    assert.deepEqual(result.tools.map((t) => t.name), ["web_search", "fetch"]);
    assert.equal(result.tools[0].title, "Web search");
    assert.equal(result.tools[0].icons[0].src, ICON_URI);
    assert.deepEqual(result.tools[0].inputSchema.required, ["query"]);
    assert.equal(result.tools[0].inputSchema.properties.limit.maximum, MAX_RESULTS);
    assert.deepEqual(result.tools[1].inputSchema.required, ["url"]);
    assert.deepEqual(result.tools[0], JSON.parse(JSON.stringify(SEARCH_TOOL)));
    assert.deepEqual(result.tools[1], JSON.parse(JSON.stringify(FETCH_TOOL)));
  });

  it("web_search: five results by default, at most ten, a numbered text", async () => {
    const five = await call("web_search", { query: "rust" });
    assert.equal(five.isError, false);
    assert.equal(five.content[0].text.split("\n").filter((l) => /^\d+\. /.test(l)).length, 5);
    const ten = await call("web_search", { query: "rust", limit: 10 });
    assert.equal(ten.content[0].text.split("\n").filter((l) => /^\d+\. /.test(l)).length, 10);
    for (const bad of [11, 0, 1.5, "3"]) {
      const answer = await call("web_search", { query: "rust", limit: bad });
      assert.equal(answer.isError, true, String(bad));
    }
  });

  it("web_search: an empty query, an empty answer and a failing provider are results the model reads", async () => {
    assert.equal((await call("web_search", { query: "  " })).isError, true);
    assert.equal((await call("web_search", {})).isError, true);
    const none = await call("web_search", { query: "nothing" });
    assert.deepEqual([none.isError, none.content[0].text], [false, "No results."]);
    const failed = await call("web_search", { query: "boom" });
    assert.equal(failed.isError, true);
    assert.match(failed.content[0].text, /HTTP 500/);
    const html = await call("web_search", { query: "html" });
    assert.match(html.content[0].text, /not JSON/);
  });

  it("fetch: the page as text with its address and title, and a refusal is an error result", async () => {
    const page = await call("fetch", { url: `${pageBase}/page` });
    assert.equal(page.isError, false);
    assert.match(page.content[0].text, new RegExp(`^URL: ${pageBase}/page\\nTitle: Fake & page\\n\\nHeading`));
    const cut = await call("fetch", { url: `${pageBase}/big` });
    assert.match(cut.content[0].text, /\[The page was cut at 64 KiB\.\]$/);
    assert.ok(Buffer.byteLength(cut.content[0].text) < MAX_TEXT_BYTES + 400);
    const refused = await call("fetch", { url: "http://169.254.169.254/latest/meta-data/" });
    assert.equal(refused.isError, true);
    assert.match(refused.content[0].text, /private or reserved/);
    assert.equal((await call("fetch", {})).isError, true);
  });

  it("an unknown tool is a protocol error, an unknown method too", async () => {
    const tool = await (await post(rpc("tools/call", { name: "nope", arguments: {} }))).json();
    assert.equal(tool.error.code, -32602);
    const method = await (await post(rpc("server/discover"))).json();
    assert.equal(method.error.code, -32601);
  });

  it("requires the bearer token, a JSON body and the right Accept; offers no GET stream", async () => {
    assert.equal((await post(rpc("ping"), { ...HEADERS, authorization: "Bearer nope" })).status, 401);
    assert.equal((await post(rpc("ping"), { ...HEADERS, authorization: "" })).status, 401);
    assert.equal((await post(rpc("ping"), { ...HEADERS, "content-type": "text/plain" })).status, 415);
    assert.equal((await post(rpc("ping"), { ...HEADERS, accept: "application/json" })).status, 406);
    assert.equal((await fetch(`${base}/mcp`, { headers: HEADERS })).status, 405);
    assert.equal((await post("{not json")).status, 400);
    assert.equal((await post(JSON.stringify([rpc("ping")]))).status, 400);
    assert.equal((await post({ jsonrpc: "2.0", method: "notifications/initialized" })).status, 202);
    assert.equal((await post(rpc("ping"), { ...HEADERS, "mcp-protocol-version": "1999-01-01" })).status, 400);
    assert.equal((await post(rpc("ping"), { ...HEADERS, origin: "http://evil.example" })).status, 403);
    assert.equal((await fetch(`${base}/healthz`)).status, 200);
  });
});
