// A small web-search MCP server for the live stack (compose.live.yaml `searxng-mcp`; dev/README.md, "Web search for
// real"). It gives an agent two tools over a search provider of your choice:
//
//   web_search { query, limit? }  titles, links and snippets from a self-hosted SearXNG (default), Brave or Tavily;
//   fetch { url }                 the readable text of one page, so that the agent can read a source and not only its snippet.
//
// It is the pattern of dev/mock-mcp-search (copy its protocol handling): MCP 2025-11-25 over Streamable HTTP, the plain,
// session-less flavour (one endpoint, POST /mcp, one JSON object per request, no SSE, no Mcp-Session-Id; GET and DELETE are
// 405). 2025-06-18 and 2025-03-26 clients are answered too. No dependencies: only node's own modules, so nothing is installed.
//
// `web_search` answers one text content: `1. <title> — <url>` and the snippet on the next line, one entry per result, at
// most 10 results, each title cut at 200 characters and each snippet at 300. A failing provider is a tool execution error
// (isError), which the model reads and can answer from; it is never a protocol error.
//
// `fetch` is the dangerous tool: the server fetches a URL a model chose, which a page it read may have written. So:
//   - http and https only, no credentials in the URL, a timeout, and at most 5 redirects;
//   - every address the name resolves to must be a public one: loopback, private (RFC 1918), link-local (the cloud metadata
//     address 169.254.169.254 among them), carrier-grade NAT, multicast, reserved and the IPv6 equivalents are refused, and the
//     check is on the address the connection is made to (a custom `lookup`), so a name that answers a public address to the
//     check and a private one to the connection (DNS rebinding) has no gap to use. A redirect is checked the same way;
//   - the body is read up to 2 MiB, only text types are read (HTML, plain text, JSON, XML), and the text handed back is cut
//     at 64 KiB, with a line saying so. Scripts, styles and comments are dropped and the HTML becomes plain text.
// The provider's own address (SEARXNG_URL: a service on the compose network) is configuration, and is not subject to that
// rule: only what a model names is.
//
// Environment: PORT (8080); SEARCH_MCP_TOKEN (the bearer token of /mcp; required, the server refuses to start without
// it unless SEARCH_MCP_NO_AUTH=true); WEB_SEARCH_PROVIDER (`searxng`, the default, `brave` or `tavily`); SEARXNG_URL (for
// searxng, e.g. http://searxng:8080); BRAVE_API_KEY and TAVILY_API_KEY (for those providers; BRAVE_API_URL and TAVILY_API_URL
// override their endpoints, for a proxy or a test).
import { createHash, timingSafeEqual } from "node:crypto";
import { lookup as dnsLookup } from "node:dns";
import http from "node:http";
import https from "node:https";
import { BlockList, isIP } from "node:net";
import { pathToFileURL } from "node:url";

/** Newest first. The version an `initialize` for any other version is answered with is the first. */
export const PROTOCOL_VERSIONS = ["2025-11-25", "2025-06-18", "2025-03-26"];
const MAX_BODY_BYTES = 64 * 1024;
const MAX_QUERY_CHARS = 500;
export const MAX_RESULTS = 10;
const DEFAULT_RESULTS = 5;
const MAX_TITLE_CHARS = 200;
const MAX_SNIPPET_CHARS = 300;
/** What `fetch` hands back, in bytes of UTF-8. */
export const MAX_TEXT_BYTES = 64 * 1024;
/** What `fetch` reads of a page before it converts it: a bigger page is cut here, then to MAX_TEXT_BYTES. */
const MAX_PAGE_BYTES = 2 * 1024 * 1024;
const MAX_REDIRECTS = 5;
const FETCH_TIMEOUT_MS = 15_000;
const SEARCH_TIMEOUT_MS = 10_000;
const MAX_PROVIDER_BYTES = 2 * 1024 * 1024;

const ICON_SVG =
  '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#4a8c4d" stroke-width="2" ' +
  'stroke-linecap="round" stroke-linejoin="round"><circle cx="11" cy="11" r="7"/><path d="m20 20-3.5-3.5"/></svg>';
export const ICON_URI = `data:image/svg+xml;base64,${Buffer.from(ICON_SVG).toString("base64")}`;
const ICONS = [{ src: ICON_URI, mimeType: "image/svg+xml", sizes: ["any"] }];

export const SEARCH_TOOL = {
  name: "web_search",
  // adam labels the step of a tool call with its `title`.
  title: "Web search",
  description:
    "Search the web. Answers a numbered list of the best matches, each with a title, a link and a short snippet. " +
    "Use `fetch` on a link to read the page itself.",
  inputSchema: {
    type: "object",
    properties: {
      query: { type: "string", minLength: 1, maxLength: MAX_QUERY_CHARS, description: "What to search for." },
      limit: {
        type: "integer",
        minimum: 1,
        maximum: MAX_RESULTS,
        description: `How many results (default ${DEFAULT_RESULTS}, at most ${MAX_RESULTS}).`,
      },
    },
    required: ["query"],
    additionalProperties: false,
  },
  annotations: { title: "Web search", readOnlyHint: true, destructiveHint: false, idempotentHint: true, openWorldHint: true },
  icons: ICONS,
};

export const FETCH_TOOL = {
  name: "fetch",
  title: "Read page",
  description:
    "Read one web page: the readable text of an http(s) link, without scripts and styles, cut at 64 KiB. " +
    "Refuses addresses on a private network.",
  inputSchema: {
    type: "object",
    properties: {
      url: { type: "string", minLength: 1, maxLength: 2048, description: "The http or https link of the page." },
    },
    required: ["url"],
    additionalProperties: false,
  },
  annotations: { title: "Read page", readOnlyHint: true, destructiveHint: false, idempotentHint: true, openWorldHint: true },
  icons: ICONS,
};

const SERVER_INFO = {
  name: "searxng-mcp",
  title: "Web search and page reader",
  version: "1.0.0",
  description: "web_search (SearXNG, Brave or Tavily) and fetch (a page as plain text) for the live stack.",
  icons: ICONS,
};

const rpcResult = (id, result) => ({ jsonrpc: "2.0", id, result });
const rpcError = (id, code, message) => ({ jsonrpc: "2.0", id, error: { code, message } });
const text = (value, isError = false) => ({ content: [{ type: "text", text: value }], isError });
const sha256 = (value) => createHash("sha256").update(value).digest();

// ------------------------------------------------------------------------------------------------ addresses

const blocked = new BlockList();
for (const [net, bits] of [
  ["0.0.0.0", 8], // "this network"
  ["10.0.0.0", 8],
  ["100.64.0.0", 10], // carrier-grade NAT
  ["127.0.0.0", 8],
  ["169.254.0.0", 16], // link-local, the cloud metadata address
  ["172.16.0.0", 12],
  ["192.0.0.0", 24],
  ["192.0.2.0", 24],
  ["192.168.0.0", 16],
  ["198.18.0.0", 15],
  ["198.51.100.0", 24],
  ["203.0.113.0", 24],
  ["224.0.0.0", 4], // multicast
  ["240.0.0.0", 4], // reserved, and the broadcast address
]) {
  blocked.addSubnet(net, bits, "ipv4");
}
for (const [net, bits] of [
  ["::", 128], // unspecified
  ["::1", 128], // loopback
  ["fc00::", 7], // unique local
  ["fe80::", 10], // link-local
  ["ff00::", 8], // multicast
  ["2001:db8::", 32], // documentation
  ["100::", 64], // discard-only
]) {
  blocked.addSubnet(net, bits, "ipv6");
}

/** The IPv4 address inside an IPv6 one that embeds it (::ffff:a.b.c.d, 64:ff9b::/96, 2002::/16), else null. */
function embeddedIpv4(address) {
  const groups = expandIpv6(address);
  if (!groups) return null;
  const v4 = (hi, lo) => `${hi >> 8}.${hi & 255}.${lo >> 8}.${lo & 255}`;
  const [g0, g1, g2, g3, g4, g5, g6, g7] = groups;
  if (g0 === 0 && g1 === 0 && g2 === 0 && g3 === 0 && g4 === 0 && g5 === 0xffff) return v4(g6, g7); // IPv4-mapped
  if (g0 === 0x64 && g1 === 0xff9b && g2 === 0 && g3 === 0 && g4 === 0 && g5 === 0) return v4(g6, g7); // NAT64
  if (g0 === 0x2002) return v4(g1, g2); // 6to4
  return null;
}

/** The eight 16-bit groups of an IPv6 address, or null when it is not one. */
function expandIpv6(address) {
  let a = address.toLowerCase().split("%")[0];
  const dotted = /(\d+\.\d+\.\d+\.\d+)$/.exec(a);
  if (dotted) {
    const parts = dotted[1].split(".").map(Number);
    if (parts.length !== 4 || parts.some((p) => !(p >= 0 && p <= 255))) return null;
    a = `${a.slice(0, -dotted[1].length)}${((parts[0] << 8) | parts[1]).toString(16)}:${((parts[2] << 8) | parts[3]).toString(16)}`;
  }
  const halves = a.split("::");
  if (halves.length > 2) return null;
  const head = halves[0] === "" ? [] : halves[0].split(":");
  const tail = halves.length === 2 && halves[1] !== "" ? halves[1].split(":") : [];
  const fill = halves.length === 2 ? 8 - head.length - tail.length : 0;
  if (fill < 0 || (halves.length === 1 && head.length !== 8)) return null;
  const groups = [...head, ...Array(fill).fill("0"), ...tail].map((g) => (/^[0-9a-f]{1,4}$/.test(g) ? parseInt(g, 16) : NaN));
  return groups.length === 8 && groups.every(Number.isInteger) ? groups : null;
}

/** True when a connection to this address must be refused: anything that is not a public unicast address. */
export function isBlockedAddress(address) {
  const family = isIP(address);
  if (family === 4) return blocked.check(address, "ipv4");
  if (family === 6) {
    const inner = embeddedIpv4(address);
    if (inner !== null && blocked.check(inner, "ipv4")) return true;
    return blocked.check(address, "ipv6");
  }
  return true; // not an address at all: fail closed
}

class RefusedError extends Error {}

/**
 * A `lookup` for http.request that resolves a name and refuses it when ANY of its addresses is blocked, so the address the
 * socket connects to is the one that was checked. `resolve` is dns.lookup (a test passes its own); `isBlocked` is the rule.
 */
export function guardedLookup({ resolve = dnsLookup, isBlocked = isBlockedAddress } = {}) {
  return (hostname, options, callback) => {
    resolve(hostname, { all: true, verbatim: true }, (error, addresses) => {
      if (error) return callback(error);
      const bad = addresses.find((a) => isBlocked(a.address));
      if (bad) return callback(new RefusedError(`${hostname} resolves to ${bad.address}, a private or reserved address`));
      if (options?.all) return callback(null, addresses);
      return callback(null, addresses[0].address, addresses[0].family);
    });
  };
}

// ------------------------------------------------------------------------------------------------ HTML to text

const NAMED_ENTITIES = {
  amp: "&", lt: "<", gt: ">", quot: '"', apos: "'", nbsp: " ", ndash: "–", mdash: "—", hellip: "…", copy: "©",
  reg: "®", laquo: "«", raquo: "»", lsquo: "‘", rsquo: "’", ldquo: "“", rdquo: "”", bull: "•", middot: "·", euro: "€",
};

function decodeEntities(value) {
  return value.replace(/&(#x[0-9a-f]+|#\d+|[a-z]+);/gi, (whole, name) => {
    if (name[0] === "#") {
      const code = name[1].toLowerCase() === "x" ? parseInt(name.slice(2), 16) : parseInt(name.slice(1), 10);
      return code > 0 && code <= 0x10ffff && !(code >= 0xd800 && code <= 0xdfff) ? String.fromCodePoint(code) : "";
    }
    return NAMED_ENTITIES[name.toLowerCase()] ?? whole;
  });
}

/** Plain text of an HTML page: no scripts, styles or comments, a line break where a block ends, entities decoded. */
export function htmlToText(html) {
  let s = html.replace(/<!--[\s\S]*?(?:-->|$)/g, " ");
  // Whole elements whose content is never prose. An unclosed one runs to the end of the page.
  s = s.replace(/<(script|style|noscript|template|svg|head)\b[^>]*>[\s\S]*?<\/\1\s*>/gi, " ");
  s = s.replace(/<(script|style|noscript|template|svg)\b[\s\S]*$/i, " ");
  const title = /<title\b[^>]*>([\s\S]*?)<\/title\s*>/i.exec(html)?.[1];
  s = s.replace(/<(?:br|hr)\b[^>]*>/gi, "\n");
  s = s.replace(/<li\b[^>]*>/gi, "\n- ");
  s = s.replace(/<\/?(?:p|div|section|article|header|footer|main|nav|aside|ul|ol|table|tr|h[1-6]|blockquote|pre|figure|figcaption|form|details|summary|dl|dt|dd)\b[^>]*>/gi, "\n");
  s = s.replace(/<\/?(?:td|th)\b[^>]*>/gi, " ");
  s = s.replace(/<[^>]*>/g, "");
  s = decodeEntities(s);
  s = s
    .replace(/\r/g, "")
    .replace(/[ \t\f\v ]+/g, " ")
    .replace(/ ?\n ?/g, "\n")
    .replace(/\n{3,}/g, "\n\n")
    .trim();
  const heading = title ? decodeEntities(title.replace(/<[^>]*>/g, "")).replace(/\s+/g, " ").trim() : "";
  return { title: heading, text: s };
}

/** At most `max` bytes of UTF-8 of `value`, never ending in the middle of a character. */
export function cutUtf8(value, max) {
  const bytes = Buffer.from(value, "utf8");
  if (bytes.length <= max) return { text: value, cut: false };
  let end = max;
  while (end > 0 && (bytes[end] & 0xc0) === 0x80) end--;
  return { text: bytes.subarray(0, end).toString("utf8"), cut: true };
}

// ------------------------------------------------------------------------------------------------ fetch

/** GET one URL, no redirects: resolves `{status, headers, body, truncated}`; the body is at most `maxBytes`. */
function getOnce(url, { lookup, timeoutMs, maxBytes, signal }) {
  return new Promise((resolve, reject) => {
    const client = url.protocol === "https:" ? https : http;
    const req = client.request(
      url,
      {
        method: "GET",
        lookup,
        signal,
        timeout: timeoutMs,
        agent: false,
        headers: {
          "user-agent": "searxng-mcp/1.0 (+another-agentic-system dev stack)",
          accept: "text/html,application/xhtml+xml,text/plain,application/json;q=0.9,*/*;q=0.1",
          "accept-encoding": "identity", // no compressed body to blow up
        },
      },
      (res) => {
        const status = res.statusCode ?? 0;
        if (status >= 300 && status < 400) {
          res.resume();
          return resolve({ status, headers: res.headers, body: Buffer.alloc(0), truncated: false });
        }
        const chunks = [];
        let size = 0;
        let truncated = false;
        res.on("data", (chunk) => {
          if (truncated) return;
          size += chunk.length;
          if (size > maxBytes) {
            chunks.push(chunk.subarray(0, chunk.length - (size - maxBytes)));
            truncated = true;
            res.destroy();
            return resolve({ status, headers: res.headers, body: Buffer.concat(chunks), truncated });
          }
          chunks.push(chunk);
        });
        res.on("end", () => resolve({ status, headers: res.headers, body: Buffer.concat(chunks), truncated }));
        res.on("error", (error) => (truncated ? undefined : reject(error)));
        res.on("close", () => resolve({ status, headers: res.headers, body: Buffer.concat(chunks), truncated }));
      },
    );
    req.on("timeout", () => req.destroy(new Error("timed out")));
    req.on("error", reject);
    req.end();
  });
}

const TEXT_TYPE = /^(text\/|application\/(xhtml\+xml|xml|json|[a-z.+-]*\+(xml|json)))/i;

/**
 * Read the page at `rawUrl` as text. Resolves `{ok: true, url, title, text, cut}` or `{ok: false, error}`; it never throws.
 * `lookup` and `isBlocked` are the address rule (a test narrows or widens them), `timeoutMs` bounds the whole call.
 */
export async function fetchPage(rawUrl, { lookup, isBlocked = isBlockedAddress, timeoutMs = FETCH_TIMEOUT_MS } = {}) {
  const guard = lookup ?? guardedLookup({ isBlocked });
  const signal = AbortSignal.timeout(timeoutMs);
  let url;
  try {
    url = new URL(rawUrl);
  } catch {
    return { ok: false, error: "`url` is not a valid URL." };
  }
  try {
    for (let hop = 0; hop <= MAX_REDIRECTS; hop++) {
      if (url.protocol !== "http:" && url.protocol !== "https:") return { ok: false, error: "Only http and https links can be read." };
      if (url.username || url.password) return { ok: false, error: "A link with a user name or password is refused." };
      // An address written in the URL is not looked up by the socket, so it is checked here.
      const host = url.hostname.startsWith("[") ? url.hostname.slice(1, -1) : url.hostname;
      if (isIP(host) && isBlocked(host)) return { ok: false, error: `Refused: ${host} is a private or reserved address.` };
      const res = await getOnce(url, { lookup: guard, timeoutMs, maxBytes: MAX_PAGE_BYTES, signal });
      if (res.status >= 300 && res.status < 400) {
        const location = res.headers.location;
        if (!location) return { ok: false, error: `HTTP ${res.status} without a Location.` };
        if (hop === MAX_REDIRECTS) return { ok: false, error: `More than ${MAX_REDIRECTS} redirects.` };
        url = new URL(location, url); // checked at the top of the next turn, like the first link
        continue;
      }
      if (res.status >= 400) return { ok: false, error: `HTTP ${res.status} for ${url.href}` };
      const type = String(res.headers["content-type"] ?? "text/html");
      if (!TEXT_TYPE.test(type)) return { ok: false, error: `Not a text page (${type.split(";")[0]}): only HTML, plain text, JSON and XML are read.` };
      let charset = /charset=["']?([\w-]+)/i.exec(type)?.[1] ?? "utf-8";
      let decoded;
      try {
        decoded = new TextDecoder(charset).decode(res.body);
      } catch {
        decoded = new TextDecoder("utf-8").decode(res.body);
      }
      const isHtml = /html/i.test(type);
      const { title, text: body } = isHtml ? htmlToText(decoded) : { title: "", text: decoded.replace(/\r/g, "").trim() };
      const { text: cutText, cut } = cutUtf8(body, MAX_TEXT_BYTES);
      return { ok: true, url: url.href, title, text: cutText, cut: cut || res.truncated };
    }
    return { ok: false, error: `More than ${MAX_REDIRECTS} redirects.` };
  } catch (error) {
    if (signal.aborted) return { ok: false, error: `Timed out after ${timeoutMs / 1000} s.` };
    const cause = error?.cause ?? error;
    if (cause instanceof RefusedError) return { ok: false, error: `Refused: ${cause.message}.` };
    return { ok: false, error: `Could not read ${url.href}: ${cause?.code ?? cause?.message ?? "error"}` };
  }
}

// ------------------------------------------------------------------------------------------------ providers

const clip = (value, max) => {
  const s = String(value ?? "").replace(/\s+/g, " ").trim();
  return s.length > max ? `${s.slice(0, max - 1)}…` : s;
};

const httpUrl = (value) => {
  try {
    const u = new URL(String(value));
    return u.protocol === "http:" || u.protocol === "https:" ? u.href : null;
  } catch {
    return null;
  }
};

/** Read a provider's JSON answer: bounded, with a timeout. Throws an Error whose message is for the model. */
async function providerJson(url, init) {
  const res = await fetch(url, { ...init, signal: AbortSignal.timeout(SEARCH_TIMEOUT_MS), redirect: "error" });
  const reader = res.body.getReader();
  const chunks = [];
  let size = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    size += value.length;
    if (size > MAX_PROVIDER_BYTES) {
      await reader.cancel();
      throw new Error("the search provider's answer is too large");
    }
    chunks.push(value);
  }
  const body = Buffer.concat(chunks).toString("utf8");
  if (!res.ok) throw new Error(`the search provider answered HTTP ${res.status}`);
  try {
    return JSON.parse(body);
  } catch {
    throw new Error("the search provider's answer is not JSON (SearXNG: is `json` among search.formats of its settings?)");
  }
}

/** Providers: each is `(query, limit) => Promise<[{title, url, snippet}]>`, from a raw list of whatever the provider calls them. */
export function createProvider(env) {
  const name = (env.WEB_SEARCH_PROVIDER ?? "searxng").toLowerCase();
  if (name === "searxng") {
    const base = env.SEARXNG_URL;
    if (!base) throw new Error("WEB_SEARCH_PROVIDER=searxng needs SEARXNG_URL (e.g. http://searxng:8080)");
    return {
      name,
      async search(query) {
        const url = new URL("search", base.endsWith("/") ? base : `${base}/`);
        url.search = new URLSearchParams({ q: query, format: "json" }).toString();
        const json = await providerJson(url, { headers: { accept: "application/json" } });
        return (json.results ?? []).map((r) => ({ title: r.title, url: r.url, snippet: r.content }));
      },
    };
  }
  if (name === "brave") {
    const key = env.BRAVE_API_KEY;
    if (!key) throw new Error("WEB_SEARCH_PROVIDER=brave needs BRAVE_API_KEY");
    const endpoint = env.BRAVE_API_URL ?? "https://api.search.brave.com/res/v1/web/search";
    return {
      name,
      async search(query, limit) {
        const url = new URL(endpoint);
        url.search = new URLSearchParams({ q: query, count: String(limit) }).toString();
        const json = await providerJson(url, { headers: { accept: "application/json", "x-subscription-token": key } });
        return (json.web?.results ?? []).map((r) => ({ title: r.title, url: r.url, snippet: r.description }));
      },
    };
  }
  if (name === "tavily") {
    const key = env.TAVILY_API_KEY;
    if (!key) throw new Error("WEB_SEARCH_PROVIDER=tavily needs TAVILY_API_KEY");
    const endpoint = env.TAVILY_API_URL ?? "https://api.tavily.com/search";
    return {
      name,
      async search(query, limit) {
        const json = await providerJson(endpoint, {
          method: "POST",
          headers: { "content-type": "application/json", accept: "application/json", authorization: `Bearer ${key}` },
          body: JSON.stringify({ query, max_results: limit }),
        });
        return (json.results ?? []).map((r) => ({ title: r.title, url: r.url, snippet: r.content }));
      },
    };
  }
  throw new Error(`WEB_SEARCH_PROVIDER must be searxng, brave or tavily, not "${name}"`);
}

/** The text `web_search` answers: numbered entries, bounded. */
export function formatResults(raw, limit) {
  const entries = [];
  for (const r of raw) {
    const url = httpUrl(r.url);
    if (!url) continue;
    const title = clip(r.title, MAX_TITLE_CHARS) || url;
    const snippet = clip(r.snippet, MAX_SNIPPET_CHARS);
    entries.push(`${entries.length + 1}. ${title} — ${url}${snippet ? `\n   ${snippet}` : ""}`);
    if (entries.length >= limit) break;
  }
  return entries.length ? entries.join("\n") : "No results.";
}

// ------------------------------------------------------------------------------------------------ the server

export function createSearchServer({ token = "", provider, fetchOptions = {}, log = () => {} }) {
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

  async function webSearch(args) {
    const query = args.query;
    if (typeof query !== "string" || query.trim() === "" || query.length > MAX_QUERY_CHARS) {
      return text(`\`query\` is required: a non-empty string of at most ${MAX_QUERY_CHARS} characters.`, true);
    }
    let limit = DEFAULT_RESULTS;
    if (args.limit !== undefined) {
      if (!Number.isInteger(args.limit) || args.limit < 1 || args.limit > MAX_RESULTS) {
        return text(`\`limit\` must be a whole number from 1 to ${MAX_RESULTS}.`, true);
      }
      limit = args.limit;
    }
    try {
      return text(formatResults(await provider.search(query.trim(), limit), limit));
    } catch (error) {
      log(`search failed: ${String(error?.message ?? error).slice(0, 200)}`);
      const why = error?.cause?.code ?? error?.cause?.errors?.[0]?.code ?? error?.message ?? "error";
      return text(`The search failed (${provider.name}): ${why}.`, true);
    }
  }

  async function fetchTool(args) {
    if (typeof args.url !== "string" || args.url.trim() === "") return text("`url` is required: the http or https link of a page.", true);
    const page = await fetchPage(args.url.trim(), fetchOptions);
    if (!page.ok) return text(page.error, true);
    const head = `URL: ${page.url}${page.title ? `\nTitle: ${page.title}` : ""}\n\n`;
    const tail = page.cut ? `\n\n[The page was cut at ${MAX_TEXT_BYTES / 1024} KiB.]` : "";
    return text(`${head}${page.text || "(The page has no text.)"}${tail}`);
  }

  async function callTool(params) {
    if (params === null || typeof params !== "object" || Array.isArray(params)) return [-32602, "Invalid params"];
    const args = params.arguments !== null && typeof params.arguments === "object" && !Array.isArray(params.arguments) ? params.arguments : {};
    if (params.name === SEARCH_TOOL.name) return webSearch(args);
    if (params.name === FETCH_TOOL.name) return fetchTool(args);
    return [-32602, `Unknown tool: ${String(params.name)}`];
  }

  /** The JSON-RPC answer to one request, `id` included. */
  async function dispatch({ id, method, params }) {
    switch (method) {
      case "initialize": {
        const requested = params?.protocolVersion;
        if (typeof requested !== "string") return rpcError(id, -32602, "Invalid params: protocolVersion is required");
        return rpcResult(id, {
          protocolVersion: PROTOCOL_VERSIONS.includes(requested) ? requested : PROTOCOL_VERSIONS[0],
          capabilities: { tools: { listChanged: false } },
          serverInfo: SERVER_INFO,
          instructions: "Two tools: web_search to find pages, fetch to read one.",
        });
      }
      case "ping":
        return rpcResult(id, {});
      case "tools/list":
        return rpcResult(id, { tools: [SEARCH_TOOL, FETCH_TOOL] });
      case "tools/call": {
        const value = await callTool(params);
        return Array.isArray(value) ? rpcError(id, value[0], value[1]) : rpcResult(id, value);
      }
      default: // `server/discover` of the 2026 revision too: a modern client then falls back to `initialize`
        return rpcError(id, -32601, `Method not found: ${method}`);
    }
  }

  async function mcp(req, res) {
    const bad = (status, message, headers) => reply(res, status, rpcError(null, -32000, message), headers);
    if (!originAllowed(req)) return bad(403, "Origin not allowed");
    if (req.method !== "POST") return bad(405, "This server offers no SSE stream and no sessions: POST only", { allow: "POST" });
    if (!authorized(req)) return bad(401, "A bearer token is required", { "www-authenticate": 'Bearer realm="searxng-mcp"' });
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
    const answer = await dispatch(message);
    // What a client sent is quoted, so that a newline in it cannot forge a line of the log. Arguments are not logged.
    const what = message.method === "tools/call" ? [message.method, message.params?.name] : [message.method];
    log(`${what.map((v) => JSON.stringify(String(v).slice(0, 64))).join(" ")} -> ${answer.error ? answer.error.code : answer.result?.isError ? "tool error" : "ok"}`);
    return reply(res, 200, answer);
  }

  return http.createServer(async (req, res) => {
    try {
      const { pathname } = new URL(req.url, "http://localhost");
      if (pathname === "/mcp") return await mcp(req, res);
      if (pathname === "/healthz" && req.method === "GET") return reply(res, 200, { status: "ok" });
      return reply(res, 404, { error: "not found" });
    } catch (error) {
      log(`error: ${error.message}`);
      return reply(res, 500, rpcError(null, -32603, "Internal error"));
    }
  });
}

function main() {
  const env = process.env;
  const port = Number(env.PORT ?? 8080);
  const token = env.SEARCH_MCP_TOKEN ?? "";
  if (!token && env.SEARCH_MCP_NO_AUTH !== "true") {
    console.error("searxng-mcp: SEARCH_MCP_TOKEN is not set (the bearer token of /mcp). Set it, or SEARCH_MCP_NO_AUTH=true for a throwaway run.");
    process.exit(78);
  }
  let provider;
  try {
    provider = createProvider(env);
  } catch (error) {
    console.error(`searxng-mcp: ${error.message}`);
    process.exit(78);
  }
  const server = createSearchServer({ token, provider, log: (line) => console.log(line) });
  server.listen(port, "0.0.0.0", () => {
    console.log(`searxng-mcp: POST /mcp on :${port}, provider ${provider.name}, ${token ? "bearer token required" : "NO AUTHENTICATION"}`);
  });
  for (const signal of ["SIGTERM", "SIGINT"]) {
    process.on(signal, () => {
      server.close(() => process.exit(0));
      server.closeAllConnections();
    });
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) main();
