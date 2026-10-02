// A mock OpenID Connect issuer for the local stack (compose.yaml `mock-oidc`; dev/README.md, "Sign in: a mock issuer and
// oauth2-proxy"). It lets a real oauth2-proxy sign a person in, and a script get a token, with no identity provider:
// every request is approved, as the user the request names or the default one. DEVELOPMENT ONLY: it authenticates nobody.
//
// What it serves (ADR 0033, the dev stack):
//   GET  /.well-known/openid-configuration   discovery; `issuer` is ISSUER, the endpoints the callers' own addresses
//   GET  /jwks, /.well-known/jwks.json       the public key, a JWK set (one RSA 2048 key made at startup, RS256)
//   GET  /authorize                          the authorization code flow with nothing to type: the user is the `login_hint`,
//                                            else the `mock_oidc_user` cookie (set by /login-as), else the default user;
//                                            302 to the `redirect_uri` with `code` and `state`. PKCE (S256, plain) is checked
//   POST /token                              grant_type=authorization_code (an ID token and an access token),
//                                            grant_type=client_credentials with the extension parameters `user` (an e-mail of
//                                            users.json, default the default user) and `audience` (default the client id): the
//                                            access token of that user, for a script. The client authenticates with
//                                            client_secret_post or client_secret_basic
//   GET  /userinfo                           the claims of the bearer access token
//   GET  /login-as?user=<e-mail>             remembers the user for the next /authorize of this browser (`user=` clears it)
//   GET  /                                   the users, with a link each; GET /healthz
// Not served: refresh tokens, consent, sessions, logout. An ID token and an access token are the same claims, signed alike.
//
// The token (an ID token and an access token alike): header {alg: RS256, kid, typ: JWT}; claims `iss` (ISSUER), `sub` (the
// user's, stable), `aud` (the client id, or the `audience` of a client_credentials request), `azp`, `iat`, `exp`
// (TOKEN_TTL_SECS after), `jti`, `email`, `email_verified: true`, `name`, `preferred_username`, the roles under
// ROLES_CLAIM (users.json: `roles`), and `nonce` when the authorize request had one.
//
// The keys are made at startup: a restart signs with a new key, and whoever cached the old one fetches the new one at its next
// unknown `kid` (the orchestrator, oauth2-proxy). Written by hand, with no dependencies, like dev/mock-mcp-search.
//
// Environment: PORT (8080); ISSUER (http://mock-oidc:8080: the `iss` of every token, and what the orchestrator and oauth2-proxy are
// configured with; discovery's `issuer`); PUBLIC_URL (the address a BROWSER reaches this server at, for discovery's
// `authorization_endpoint`; default ISSUER); CLIENT_ID (dev-chat), CLIENT_SECRET (dev-client-secret; empty: none is asked);
// TOKEN_TTL_SECS (3600: the AG-UI streams of the orchestrator end at the token's `exp` plus 60 s and after an hour at most, so an
// hour is the longest stream there is); ROLES_CLAIM (roles); MOCK_OIDC_USERS (a path; default users.json beside this file).
import {
  createHash,
  createSign,
  createVerify,
  generateKeyPairSync,
  randomBytes,
  timingSafeEqual,
} from "node:crypto";
import { readFileSync } from "node:fs";
import { createServer } from "node:http";
import { pathToFileURL } from "node:url";

const MAX_BODY_BYTES = 16 * 1024;
const CODE_TTL_MS = 60 * 1000;
const MAX_CODES = 1000;
const USER_COOKIE = "mock_oidc_user";

const b64url = (buffer) => Buffer.from(buffer).toString("base64url");

/** `a` and `b` are equal, in constant time (a length that differs is not equal). */
function same(a, b) {
  const x = createHash("sha256").update(String(a)).digest();
  const y = createHash("sha256").update(String(b)).digest();
  return timingSafeEqual(x, y);
}

/** A stable subject for an e-mail: opaque, not the e-mail, so a client that mistakes `sub` for it is found out. */
export const subjectOf = (email) => `mock-${createHash("sha256").update(email).digest("hex").slice(0, 16)}`;

/** The keys of the process: an RSA 2048 pair, its JWK (RFC 7638 thumbprint for `kid`) and a signer. */
export function makeKeys() {
  const { publicKey, privateKey } = generateKeyPairSync("rsa", { modulusLength: 2048 });
  const { kty, n, e } = publicKey.export({ format: "jwk" });
  const kid = b64url(createHash("sha256").update(JSON.stringify({ e, kty, n })).digest()).slice(0, 22);
  const jwk = { kty, n, e, kid, use: "sig", alg: "RS256" };
  const sign = (claims) => {
    const head = b64url(JSON.stringify({ alg: "RS256", kid, typ: "JWT" }));
    const body = b64url(JSON.stringify(claims));
    const signature = createSign("RSA-SHA256").update(`${head}.${body}`).sign(privateKey);
    return `${head}.${body}.${b64url(signature)}`;
  };
  const verify = (token) => {
    const parts = String(token).split(".");
    if (parts.length !== 3) return null;
    const ok = createVerify("RSA-SHA256").update(`${parts[0]}.${parts[1]}`).verify(publicKey, Buffer.from(parts[2], "base64url"));
    if (!ok) return null;
    try {
      return JSON.parse(Buffer.from(parts[1], "base64url").toString("utf8"));
    } catch {
      return null;
    }
  };
  return { jwk, sign, verify };
}

function readBody(req) {
  return new Promise((resolve, reject) => {
    const chunks = [];
    let size = 0;
    req.on("data", (chunk) => {
      size += chunk.length;
      if (size > MAX_BODY_BYTES) {
        reject(Object.assign(new Error("body too large"), { status: 413 }));
        req.destroy();
      } else {
        chunks.push(chunk);
      }
    });
    req.on("end", () => resolve(Buffer.concat(chunks).toString("utf8")));
    req.on("error", reject);
  });
}

function reply(res, status, body, headers = {}) {
  const isJson = body !== undefined && typeof body === "object";
  const text = body === undefined ? "" : isJson ? JSON.stringify(body) : String(body);
  res.writeHead(status, {
    "content-type": isJson ? "application/json" : "text/plain; charset=utf-8",
    "cache-control": "no-store",
    ...headers,
  });
  res.end(text);
}

const oauthError = (res, status, error, description, headers = {}) =>
  reply(res, status, { error, error_description: description }, headers);

function cookieOf(req, name) {
  for (const part of String(req.headers.cookie ?? "").split(";")) {
    const [key, ...rest] = part.trim().split("=");
    if (key === name) {
      try {
        return decodeURIComponent(rest.join("="));
      } catch {
        return "";
      }
    }
  }
  return "";
}

/** The client's id and secret of a token request: client_secret_basic (RFC 6749 2.3.1, form-encoded) beside client_secret_post. */
function clientOf(req, form) {
  const header = String(req.headers.authorization ?? "");
  if (/^basic /i.test(header)) {
    const decoded = Buffer.from(header.slice(6).trim(), "base64").toString("utf8");
    const at = decoded.indexOf(":");
    if (at > 0) {
      const unform = (v) => decodeURIComponent(v.replace(/\+/g, " "));
      try {
        return { id: unform(decoded.slice(0, at)), secret: unform(decoded.slice(at + 1)) };
      } catch {
        return { id: "", secret: "" };
      }
    }
  }
  return { id: form.get("client_id") ?? "", secret: form.get("client_secret") ?? "" };
}

/**
 * The server, as a function of its configuration so that the test can start one with its own: `users` is the parsed users.json,
 * `issuer` the `iss`, `publicUrl` where a browser reaches it, `now` the clock in seconds.
 */
export function createMockServer({
  users,
  issuer = "http://mock-oidc:8080",
  publicUrl = issuer,
  clientId = "dev-chat",
  clientSecret = "dev-client-secret",
  ttlSecs = 3600,
  rolesClaim = "roles",
  now = () => Math.floor(Date.now() / 1000),
  log = () => {},
}) {
  issuer = issuer.replace(/\/+$/, "");
  publicUrl = publicUrl.replace(/\/+$/, "");
  const table = users.users ?? {};
  const defaultUser = users.defaultUser ?? Object.keys(table)[0];
  const known = (email) => typeof email === "string" && Object.hasOwn(table, email.trim().toLowerCase());
  const keys = makeKeys();
  const codes = new Map();

  function token(email, { audience, nonce } = {}) {
    const user = table[email];
    const iat = now();
    const claims = {
      iss: issuer,
      sub: subjectOf(email),
      aud: audience ?? clientId,
      azp: clientId,
      iat,
      exp: iat + ttlSecs,
      jti: b64url(randomBytes(12)),
      email,
      email_verified: true,
      name: user.name ?? email,
      preferred_username: email,
      [rolesClaim]: user.roles ?? [],
    };
    if (nonce) claims.nonce = nonce;
    return keys.sign(claims);
  }

  const clientIsRight = ({ id, secret }) =>
    (id === "" || id === clientId) && (clientSecret === "" || same(secret, clientSecret));

  function discovery() {
    return {
      issuer,
      authorization_endpoint: `${publicUrl}/authorize`,
      token_endpoint: `${issuer}/token`,
      userinfo_endpoint: `${issuer}/userinfo`,
      jwks_uri: `${issuer}/jwks`,
      response_types_supported: ["code"],
      grant_types_supported: ["authorization_code", "client_credentials"],
      subject_types_supported: ["public"],
      id_token_signing_alg_values_supported: ["RS256"],
      token_endpoint_auth_methods_supported: ["client_secret_basic", "client_secret_post"],
      code_challenge_methods_supported: ["S256", "plain"],
      scopes_supported: ["openid", "email", "profile"],
      claims_supported: ["sub", "iss", "aud", "exp", "iat", "email", "email_verified", "name", "preferred_username", rolesClaim],
    };
  }

  function authorize(req, res, url) {
    const q = url.searchParams;
    const redirectUri = q.get("redirect_uri") ?? "";
    if (q.get("client_id") !== clientId) return reply(res, 400, `unknown client_id (this issuer has one: ${clientId})`);
    let target;
    try {
      target = new URL(redirectUri);
    } catch {
      return reply(res, 400, "redirect_uri is missing or not a URL");
    }
    if (target.protocol !== "http:" && target.protocol !== "https:") return reply(res, 400, "redirect_uri must be http or https");
    if (q.get("response_type") !== "code") return reply(res, 400, "response_type must be code");
    const hint = (q.get("login_hint") ?? "").trim().toLowerCase();
    const remembered = cookieOf(req, USER_COOKIE).trim().toLowerCase();
    const email = hint || (known(remembered) ? remembered : defaultUser);
    if (!known(email)) {
      return reply(res, 400, `login_hint names ${JSON.stringify(email)}, which is not a user of this issuer (users: ${Object.keys(table).join(", ")})`);
    }
    if (codes.size >= MAX_CODES) codes.delete(codes.keys().next().value);
    const code = b64url(randomBytes(24));
    codes.set(code, {
      email,
      redirectUri,
      nonce: q.get("nonce") ?? "",
      challenge: q.get("code_challenge") ?? "",
      method: q.get("code_challenge_method") ?? "plain",
      expires: Date.now() + CODE_TTL_MS,
    });
    target.searchParams.set("code", code);
    if (q.has("state")) target.searchParams.set("state", q.get("state"));
    target.searchParams.set("iss", issuer);
    log(`authorize: ${email} (${hint ? "login_hint" : remembered === email ? "cookie" : "default"})`);
    return reply(res, 302, undefined, { location: target.toString() });
  }

  async function tokenEndpoint(req, res) {
    let form;
    try {
      form = new URLSearchParams(await readBody(req));
    } catch (error) {
      return oauthError(res, error.status === 413 ? 413 : 400, "invalid_request", "the body could not be read");
    }
    const client = clientOf(req, form);
    if (!clientIsRight(client)) {
      return oauthError(res, 401, "invalid_client", "wrong client_id or client_secret", { "www-authenticate": 'Basic realm="mock-oidc"' });
    }
    const grant = form.get("grant_type");
    if (grant === "client_credentials") {
      const email = (form.get("user") ?? defaultUser).trim().toLowerCase();
      if (!known(email)) return oauthError(res, 400, "invalid_request", `user ${JSON.stringify(email)} is not a user of this issuer`);
      const audience = form.get("audience") || undefined;
      log(`client_credentials: ${email}, aud ${audience ?? clientId}`);
      const access = token(email, { audience });
      return reply(res, 200, { access_token: access, id_token: access, token_type: "Bearer", expires_in: ttlSecs, scope: "openid email profile" });
    }
    if (grant === "authorization_code") {
      const code = form.get("code") ?? "";
      const grantRecord = codes.get(code);
      codes.delete(code); // single use, whatever follows
      if (!grantRecord || grantRecord.expires < Date.now()) return oauthError(res, 400, "invalid_grant", "the code is unknown, used or expired");
      if (form.has("redirect_uri") && form.get("redirect_uri") !== grantRecord.redirectUri) {
        return oauthError(res, 400, "invalid_grant", "redirect_uri is not the one of the authorize request");
      }
      if (grantRecord.challenge) {
        const verifier = form.get("code_verifier") ?? "";
        const expected = grantRecord.method === "S256" ? b64url(createHash("sha256").update(verifier).digest()) : verifier;
        if (!verifier || !same(expected, grantRecord.challenge)) return oauthError(res, 400, "invalid_grant", "code_verifier does not match code_challenge");
      }
      log(`token: ${grantRecord.email}, authorization_code`);
      return reply(res, 200, {
        access_token: token(grantRecord.email),
        id_token: token(grantRecord.email, { nonce: grantRecord.nonce }),
        token_type: "Bearer",
        expires_in: ttlSecs,
        scope: "openid email profile",
      });
    }
    return oauthError(res, 400, "unsupported_grant_type", "authorization_code and client_credentials only");
  }

  function userinfo(req, res) {
    const header = String(req.headers.authorization ?? "");
    const claims = /^bearer /i.test(header) ? keys.verify(header.slice(7).trim()) : null;
    if (!claims || claims.exp <= now()) return reply(res, 401, { error: "invalid_token" }, { "www-authenticate": 'Bearer error="invalid_token"' });
    const { sub, email, email_verified, name, preferred_username } = claims;
    return reply(res, 200, { sub, email, email_verified, name, preferred_username, [rolesClaim]: claims[rolesClaim] ?? [] });
  }

  function loginAs(res, url) {
    const email = (url.searchParams.get("user") ?? "").trim().toLowerCase();
    if (email === "") {
      return reply(res, 200, "The next sign-in is as the default user again.\n", {
        "set-cookie": `${USER_COOKIE}=; Path=/; Max-Age=0; SameSite=Lax`,
      });
    }
    if (!known(email)) return reply(res, 400, `${JSON.stringify(email)} is not a user of this issuer (users: ${Object.keys(table).join(", ")})`);
    return reply(res, 200, `The next sign-in is as ${email}. Sign out first (/oauth2/sign_out on the edge) if you are signed in.\n`, {
      "set-cookie": `${USER_COOKIE}=${encodeURIComponent(email)}; Path=/; SameSite=Lax`,
    });
  }

  function index(res) {
    const rows = Object.entries(table).map(([email, u]) => `  ${email}  roles: ${(u.roles ?? []).join(", ") || "(none)"}  /login-as?user=${email}`);
    return reply(res, 200, `mock-oidc, issuer ${issuer}\ndefault user: ${defaultUser}\n${rows.join("\n")}\n`);
  }

  return createServer(async (req, res) => {
    try {
      const url = new URL(req.url, "http://localhost");
      const { pathname } = url;
      const get = req.method === "GET" || req.method === "HEAD";
      if (pathname === "/.well-known/openid-configuration" && get) return reply(res, 200, discovery());
      if ((pathname === "/jwks" || pathname === "/.well-known/jwks.json") && get) return reply(res, 200, { keys: [keys.jwk] });
      if (pathname === "/authorize" && get) return authorize(req, res, url);
      if (pathname === "/token") {
        if (req.method !== "POST") return reply(res, 405, "POST", { allow: "POST" });
        return await tokenEndpoint(req, res);
      }
      if (pathname === "/userinfo" && get) return userinfo(req, res);
      if (pathname === "/login-as" && get) return loginAs(res, url);
      if (pathname === "/healthz" && get) return reply(res, 200, { status: "ok" });
      if (pathname === "/" && get) return index(res);
      return reply(res, 404, { error: "not found" });
    } catch (error) {
      log(`error: ${error.message}`);
      return reply(res, 500, { error: "server_error" });
    }
  });
}

function main() {
  const port = Number(process.env.PORT ?? 8080);
  const issuer = process.env.ISSUER ?? "http://mock-oidc:8080";
  const file = process.env.MOCK_OIDC_USERS ?? new URL("./users.json", import.meta.url);
  const users = JSON.parse(readFileSync(file, "utf8"));
  const server = createMockServer({
    users,
    issuer,
    publicUrl: process.env.PUBLIC_URL || issuer,
    clientId: process.env.CLIENT_ID ?? "dev-chat",
    clientSecret: process.env.CLIENT_SECRET ?? "dev-client-secret",
    ttlSecs: Number(process.env.TOKEN_TTL_SECS ?? 3600),
    rolesClaim: process.env.ROLES_CLAIM ?? "roles",
    log: (line) => console.log(line),
  });
  server.listen(port, "0.0.0.0", () => {
    console.log(`mock-oidc: issuer ${issuer} on :${port}, ${Object.keys(users.users).length} users, tokens last ${process.env.TOKEN_TTL_SECS ?? 3600} s`);
  });
  for (const signal of ["SIGTERM", "SIGINT"]) {
    process.on(signal, () => {
      server.close(() => process.exit(0));
      server.closeAllConnections();
    });
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) main();
