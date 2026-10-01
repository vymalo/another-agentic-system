// A mock web-search MCP server for the local stack (compose.yaml `mock-mcp-search`; dev/README.md,
// "Mock web search (MCP)"). It lets an agent that searches the web, and the "web search attached to a chat"
// scenario, run offline and deterministically: the answers are canned, from results.json.
//
// Protocol: MCP 2025-11-25 over Streamable HTTP, the plain, session-less flavour. One endpoint, POST /mcp, one
// JSON object per request (never an SSE stream), no Mcp-Session-Id; GET and DELETE on it are 405, which the
// transport spec reserves for a server that offers no SSE stream and no sessions. 2025-06-18 and 2025-03-26
// clients are answered too. The 2026-07-28 revision (no `initialize`, `server/discover`) is not: a client that
// probes for it is refused (400 for its MCP-Protocol-Version, -32601 without it) and falls back to `initialize`.
//
// Written by hand, with no dependencies, instead of with the official SDK (91 packages, 29 MB and a lockfile
// to keep, for the same protocol revision). server.test.mjs and dev/check-agent-mocks.sh hold it to the
// protocol, and it was run against two independent clients: rmcp 3.5 (adam-rs's adam-mcp, and the one the
// orchestrator uses) and the official TypeScript SDK's.
//
// One tool, `web_search { query }`, with an icon (MCP 2025-11-25 `icons`: a data: URI), answering one text
// content: `1. <title> — <url>` and the snippet on the next line, one entry per result.
//   - the first keyword of results.json (case-insensitive, in file order) found in the query picks the list,
//     else `default`;
//   - `[mock:empty]` in the query answers "No results.", `[mock:error]` a tool execution error (isError);
//   - an empty or missing `query` is also a tool execution error (the spec's way to let a model self-correct).
// Other routes (no authentication, like WireMock's admin): GET /healthz; GET /__journal lists the calls of
// the tool, `{"calls": [{"tool", "arguments", "at"}]}`, and DELETE /__journal empties it.
//
// Environment: PORT (8080), MOCK_MCP_TOKEN (when set, `Authorization: Bearer <it>` is required on /mcp, else
// 401), MOCK_MCP_RESULTS (a path; default results.json beside this file).
import { createHash, timingSafeEqual } from "node:crypto";
import { readFileSync } from "node:fs";
import { createServer } from "node:http";
import { pathToFileURL } from "node:url";

/** Newest first. The version an `initialize` for any other version is answered with is the first. */
export const PROTOCOL_VERSIONS = ["2025-11-25", "2025-06-18", "2025-03-26"];
const MAX_BODY_BYTES = 64 * 1024;
const MAX_JOURNAL = 1000;
const MAX_QUERY_CHARS = 500;

const ICON_SVG =
  '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#4a8c4d" stroke-width="2" ' +
  'stroke-linecap="round" stroke-linejoin="round"><circle cx="11" cy="11" r="7"/><path d="m20 20-3.5-3.5"/></svg>';
export const ICON_URI = `data:image/svg+xml;base64,${Buffer.from(ICON_SVG).toString("base64")}`;
const ICONS = [{ src: ICON_URI, mimeType: "image/svg+xml", sizes: ["any"] }];

export const TOOL = {
  name: "web_search",
  title: "Web search",
  description:
    "Search the web. Answers a numbered list of the best matches, each with a title, a link and a short snippet.",
  inputSchema: {
    type: "object",
    properties: {
      query: { type: "string", minLength: 1, maxLength: MAX_QUERY_CHARS, description: "What to search for." },
    },
    required: ["query"],
    additionalProperties: false,
  },
  annotations: { title: "Web search", readOnlyHint: true, destructiveHint: false, idempotentHint: true, openWorldHint: true },
  icons: ICONS,
};

const SERVER_INFO = {
  name: "mock-mcp-search",
  title: "Mock web search",
  version: "1.0.0",
  description: "Canned, deterministic web-search results for the local stack.",
  icons: ICONS,
};

const rpcResult = (id, result) => ({ jsonrpc: "2.0", id, result });
const rpcError = (id, code, message) => ({ jsonrpc: "2.0", id, error: { code, message } });
const text = (value, isError = false) => ({ content: [{ type: "text", text: value }], isError });
const sha256 = (value) => createHash("sha256").update(value).digest();

/** Check the shape of a results file and return it: a failure at startup beats a confusing answer later. */
export function validateResults(results) {
  const entry = (r) => r && ["title", "url", "snippet"].every((k) => typeof r[k] === "string" && r[k] !== "");
  const list = (l) => Array.isArray(l) && l.length > 0 && l.every(entry);
  if (!list(results?.default)) throw new Error("results: `default` must be a non-empty list of {title, url, snippet}");
  for (const [keyword, l] of Object.entries(results.keywords ?? {})) {
    if (!list(l)) throw new Error(`results: keyword "${keyword}" must be a non-empty list of {title, url, snippet}`);
  }
  return results;
}

export function createMockServer({ token = "", results, log = () => {} }) {
  validateResults(results);
  const keywords = Object.entries(results.keywords ?? {}).map(([k, list]) => [k.toLowerCase(), list]);
  const journal = [];
  const expected = token ? sha256(token) : null;

  function reply(res, status, body, headers = {}) {
    if (body === undefined) {
      res.writeHead(status, headers);
      return res.end();
    }
    res.writeHead(status, { "content-type": "application/json", "cache-control": "no-store", ...headers });
    return res.end(JSON.stringify(body));
  }

  const authorized = (req) => {
    if (!expected) return true;
    const match = /^Bearer\s+(\S+)$/i.exec(req.headers.authorization ?? "");
    return match !== null && timingSafeEqual(sha256(match[1]), expected);
  };

  // The spec's DNS-rebinding guard: a request that names an Origin must come from a page of this very host.
  const originAllowed = (req) => {
    const origin = req.headers.origin;
    if (origin === undefined) return true;
    try {
      return new URL(origin).host === req.headers.host;
    } catch {
      return false;
    }
  };

  const accepts = (req, type) => {
    const listed = (req.headers.accept ?? "").split(",").map((v) => v.split(";")[0].trim().toLowerCase());
    return listed.some((v) => v === type || v === "*/*" || v === `${type.split("/")[0]}/*`);
  };

  async function readBody(req) {
    const chunks = [];
    let size = 0;
    for await (const chunk of req) {
      size += chunk.length;
      if (size > MAX_BODY_BYTES) throw Object.assign(new Error("body too large"), { status: 413 });
      chunks.push(chunk);
    }
    return Buffer.concat(chunks).toString("utf8");
  }

  function search(query) {
    const lowered = query.toLowerCase();
    const found = keywords.find(([keyword]) => lowered.includes(keyword));
    const list = found ? found[1] : results.default;
    return list.map((r, i) => `${i + 1}. ${r.title} — ${r.url}\n   ${r.snippet}`).join("\n");
  }

  function callTool(params) {
    if (params === null || typeof params !== "object" || Array.isArray(params)) return [-32602, "Invalid params"];
    if (params.name !== TOOL.name) return [-32602, `Unknown tool: ${String(params.name)}`];
    const args = params.arguments ?? {};
    journal.push({ tool: TOOL.name, arguments: args, at: new Date().toISOString() });
    if (journal.length > MAX_JOURNAL) journal.shift();
    const query = args !== null && typeof args === "object" ? args.query : undefined;
    if (typeof query !== "string" || query.trim() === "" || query.length > MAX_QUERY_CHARS) {
      return text(`\`query\` is required: a non-empty string of at most ${MAX_QUERY_CHARS} characters.`, true);
    }
    if (query.includes("[mock:error]")) return text("The search service failed (mock error).", true);
    if (query.includes("[mock:empty]")) return text("No results.");
    return text(search(query));
  }

  /** The JSON-RPC answer to one request, `id` included. */
  function dispatch({ id, method, params }) {
    const outcome = (value) =>
      Array.isArray(value) ? rpcError(id, value[0], value[1]) : rpcResult(id, value);
    switch (method) {
      case "initialize": {
        const requested = params?.protocolVersion;
        if (typeof requested !== "string") return rpcError(id, -32602, "Invalid params: protocolVersion is required");
        return rpcResult(id, {
          protocolVersion: PROTOCOL_VERSIONS.includes(requested) ? requested : PROTOCOL_VERSIONS[0],
          capabilities: { tools: { listChanged: false } },
          serverInfo: SERVER_INFO,
          instructions: "One tool, web_search. It answers canned results: this is a mock for tests.",
        });
      }
      case "ping":
        return rpcResult(id, {});
      case "tools/list":
        return rpcResult(id, { tools: [TOOL] });
      case "tools/call":
        return outcome(callTool(params));
      default: // `server/discover` of the 2026 revision too: a modern client then falls back to `initialize`
        return rpcError(id, -32601, `Method not found: ${method}`);
    }
  }

  async function mcp(req, res) {
    const bad = (status, message, headers) => reply(res, status, rpcError(null, -32000, message), headers);
    if (!originAllowed(req)) return bad(403, "Origin not allowed");
    if (req.method !== "POST") return bad(405, "This server offers no SSE stream and no sessions: POST only", { allow: "POST" });
    if (!authorized(req)) return bad(401, "A bearer token is required", { "www-authenticate": 'Bearer realm="mock-mcp-search"' });
    if (!/^application\/json\s*(;|$)/i.test(req.headers["content-type"] ?? "")) return bad(415, "Content-Type must be application/json");
    if (!accepts(req, "application/json") || !accepts(req, "text/event-stream")) {
      return bad(406, "Accept must list application/json and text/event-stream");
    }
    const version = req.headers["mcp-protocol-version"];
    if (version !== undefined && !PROTOCOL_VERSIONS.includes(version)) return bad(400, `Unsupported MCP-Protocol-Version: ${version}`);

    let message;
    try {
      message = JSON.parse(await readBody(req));
    } catch (error) {
      if (error.status === 413) return bad(413, "Body too large", { connection: "close" });
      return reply(res, 400, rpcError(null, -32700, "Parse error"));
    }
    if (message === null || typeof message !== "object" || Array.isArray(message) || message.jsonrpc !== "2.0") {
      return reply(res, 400, rpcError(null, -32600, "Invalid request: one JSON-RPC 2.0 object per POST (no batches)"));
    }
    const hasId = typeof message.id === "string" || typeof message.id === "number";
    const hasMethod = typeof message.method === "string";
    if (!(hasMethod && hasId)) {
      // A notification, or a client's answer to a server request (this server sends none): accepted, no body.
      const notification = hasMethod && !("id" in message);
      const response = !hasMethod && hasId && ("result" in message || "error" in message);
      return notification || response ? reply(res, 202) : reply(res, 400, rpcError(null, -32600, "Invalid request"));
    }
    const answer = dispatch(message);
    // What a client sent is quoted, so that a newline in it cannot forge a line of the log.
    const what = message.method === "tools/call" ? [message.method, message.params?.name] : [message.method];
    log(`${what.map((v) => JSON.stringify(String(v).slice(0, 64))).join(" ")} -> ${answer.error ? answer.error.code : "ok"}`);
    return reply(res, 200, answer);
  }

  return createServer(async (req, res) => {
    try {
      const { pathname } = new URL(req.url, "http://localhost");
      if (pathname === "/mcp") return await mcp(req, res);
      if (pathname === "/healthz" && req.method === "GET") return reply(res, 200, { status: "ok" });
      if (pathname === "/__journal") {
        if (req.method === "GET") return reply(res, 200, { calls: journal });
        if (req.method === "DELETE") {
          journal.length = 0;
          return reply(res, 200, { calls: [] });
        }
        return reply(res, 405, { error: "GET or DELETE" }, { allow: "GET, DELETE" });
      }
      return reply(res, 404, { error: "not found" });
    } catch (error) {
      log(`error: ${error.message}`);
      return reply(res, 500, rpcError(null, -32603, "Internal error"));
    }
  });
}

function main() {
  const port = Number(process.env.PORT ?? 8080);
  const token = process.env.MOCK_MCP_TOKEN ?? "";
  const file = process.env.MOCK_MCP_RESULTS ?? new URL("./results.json", import.meta.url);
  const results = JSON.parse(readFileSync(file, "utf8"));
  const server = createMockServer({ token, results, log: (line) => console.log(line) });
  server.listen(port, "0.0.0.0", () => {
    console.log(`mock-mcp-search: POST /mcp on :${port}, ${token ? "bearer token required" : "no authentication"}`);
  });
  for (const signal of ["SIGTERM", "SIGINT"]) {
    process.on(signal, () => {
      server.close(() => process.exit(0));
      server.closeAllConnections();
    });
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) main();
