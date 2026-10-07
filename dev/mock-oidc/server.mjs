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
//   GET  /userinfo                           the claims of the bearer access token (a DPoP-bound one: `Authorization: DPoP` and a proof)
//   GET  /login-as?user=<e-mail>             remembers the user for the next /authorize of this browser (`user=` clears it)
//   GET  /                                   the users, with a link each; GET /healthz
//
// The public client of the browser web (ADR 0054; `WEB_CLIENT_ID`, default `dev-web`, empty: off), which mirrors the Keycloak client
// `another-agentic-web` (deploy/keycloak/client-another-agentic-web.json): no secret, authorization code with PKCE S256 (REQUIRED),
// every token DPoP-bound (RFC 9449), an access token of WEB_TOKEN_TTL_SECS (300), the audience mapper (`aud` is the web client
// and the confidential client `CLIENT_ID`, so the orchestrator's `auth.jwt.audiences` holds) and the same roles claim:
//   GET  /authorize                          client_id=dev-web: `code_challenge` with S256 is required, `scope` is read (`offline_access` asks
//                                            for an offline refresh token), `dpop_jkt` binds the code to a key (RFC 9449 section 10)
//   POST /token                              authorization_code and refresh_token with a `DPoP` proof header: typ dpop+jwt, alg ES256, a
//                                            public `jwk` (no private member), htm POST, htu the token endpoint, iat within the window
//                                            (DPOP_LIFETIME_SECS 10 plus DPOP_SKEW_SECS 15 back, DPOP_SKEW_SECS ahead), jti used once.
//                                            The access token carries `cnf.jkt` (RFC 7638 thumbprint) and the refresh token is bound to the
//                                            same key: a refresh with no proof or another key's is `invalid_grant`. No `DPoP-Nonce`, like
//                                            Keycloak. `token_type` is `DPoP`
//                                            A refresh token is good ONCE (Keycloak's "Revoke Refresh Token" with a reuse of 0): the second
//                                            use of one (with the right key) is `invalid_grant` and ends the whole session (family), the
//                                            new token included. A refresh token of an offline session lives REFRESH_OFFLINE_SECS (30 days,
//                                            Keycloak's Offline Session Idle) from its last use, any other REFRESH_SESSION_SECS (30 minutes)
//   POST /revoke                             RFC 7009: the refresh token (a bound one needs a proof of its key) and with it the session
//   GET  /logout                             the end_session endpoint: forgets the remembered user and redirects to `post_logout_redirect_uri`
//                                            (the mock keeps no SSO session, and, as in Keycloak, an offline token survives it)
// CORS (`BROWSER_ORIGINS`, a comma list of origins) on discovery, the keys, token, revoke and userinfo, preflight included, for the
// origins listed and no others; no header is exposed beyond the safelisted ones (so a page cannot read `Date` of an answer: unverified
// for Keycloak). `WEB_REDIRECT_URIS` (a comma list, default any http(s) URI, as for the confidential client) is the exact list of redirect URIs.
// Not served: consent, SSO sessions, access-token revocation (a token is valid until it expires). An ID token and an access token are the
// same claims, signed alike.
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
// `authorization_endpoint` and `end_session_endpoint`; default ISSUER); CLIENT_ID (dev-chat), CLIENT_SECRET (dev-client-secret; empty: none is asked);
// TOKEN_TTL_SECS (3600: the AG-UI streams of the orchestrator end at the token's `exp` plus 60 s and after an hour at most, so an
// hour is the longest stream there is); ROLES_CLAIM (roles); MOCK_OIDC_USERS (a path; default users.json beside this file);
// the public client: WEB_CLIENT_ID (dev-web; empty: no public client), WEB_TOKEN_TTL_SECS (300), WEB_REDIRECT_URIS (default: any),
// BROWSER_ORIGINS (default: none), DPOP_LIFETIME_SECS (10), DPOP_SKEW_SECS (15), REFRESH_SESSION_SECS (1800), REFRESH_OFFLINE_SECS (2592000).
import {
  createHash,
  createPublicKey,
  createSign,
  createVerify,
  generateKeyPairSync,
  randomBytes,
  timingSafeEqual,
  verify as verifySignature,
} from "node:crypto";
import { readFileSync } from "node:fs";
import { createServer } from "node:http";
import { pathToFileURL } from "node:url";

const MAX_BODY_BYTES = 16 * 1024;
const CODE_TTL_MS = 60 * 1000;
const MAX_CODES = 1000;
const MAX_REFRESH_TOKENS = 5000;
const MAX_PROOFS = 5000;
const USER_COOKIE = "mock_oidc_user";
const WEB_SCOPES = ["openid", "email", "profile", "offline_access"];
// What a page may call across origins: the endpoints a browser client uses.
const CORS_PATHS = new Set(["/.well-known/openid-configuration", "/jwks", "/.well-known/jwks.json", "/token", "/revoke", "/userinfo"]);

const b64url = (buffer) => Buffer.from(buffer).toString("base64url");

/** `a` and `b` are equal, in constant time (a length that differs is not equal). */
function same(a, b) {
  const x = createHash("sha256").update(String(a)).digest();
  const y = createHash("sha256").update(String(b)).digest();
  return timingSafeEqual(x, y);
}

/** RFC 7638 thumbprint of a public EC P-256 JWK: the SHA-256 of its required members in lexicographic order, base64url. */
export const thumbprint = ({ crv, kty, x, y }) => b64url(createHash("sha256").update(JSON.stringify({ crv, kty, x, y })).digest());

/** A URL as RFC 9449 compares `htu`: without its query and fragment. */
function withoutQuery(value) {
  const url = new URL(value);
  return `${url.origin}${url.pathname}`;
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
  // The public client of the browser web (ADR 0054); an empty id turns it off.
  webClientId = "dev-web",
  webTtlSecs = 300,
  webRedirectUris = [],
  browserOrigins = [],
  dpopLifetimeSecs = 10,
  dpopSkewSecs = 15,
  refreshSessionSecs = 1800,
  refreshOfflineSecs = 30 * 24 * 3600,
}) {
  issuer = issuer.replace(/\/+$/, "");
  publicUrl = publicUrl.replace(/\/+$/, "");
  const table = users.users ?? {};
  const defaultUser = users.defaultUser ?? Object.keys(table)[0];
  const known = (email) => typeof email === "string" && Object.hasOwn(table, email.trim().toLowerCase());
  const keys = makeKeys();
  const codes = new Map();
  const refreshTokens = new Map(); // a refresh token of the public client -> { session, used, expires }; a session is { client, email, jkt, scope, offline, revoked }
  const proofs = new Map(); // a DPoP proof's jti -> until when it is remembered
  const isWeb = (id) => webClientId !== "" && id === webClientId;
  const tokenUrls = [...new Set([`${issuer}/token`, `${publicUrl}/token`])];
  const revokeUrls = [...new Set([`${issuer}/revoke`, `${publicUrl}/revoke`])];
  const userinfoUrls = [...new Set([`${issuer}/userinfo`, `${publicUrl}/userinfo`])];

  function token(email, { audience, audiences, nonce, azp, ttl, jkt } = {}) {
    const user = table[email];
    const iat = now();
    const claims = {
      iss: issuer,
      sub: subjectOf(email),
      aud: audiences ?? audience ?? clientId,
      azp: azp ?? clientId,
      iat,
      exp: iat + (ttl ?? ttlSecs),
      jti: b64url(randomBytes(12)),
      email,
      email_verified: true,
      name: user.name ?? email,
      preferred_username: email,
      [rolesClaim]: user.roles ?? [],
    };
    if (nonce) claims.nonce = nonce;
    if (jkt) claims.cnf = { jkt };
    return keys.sign(claims);
  }

  /** A token of the public client: the web client and the confidential client in `aud` (the audience mapper), bound to a key. */
  const webToken = (email, { nonce, jkt } = {}) =>
    token(email, { audiences: [webClientId, clientId], azp: webClientId, ttl: webTtlSecs, nonce, jkt });

  /**
   * Checks a DPoP proof (RFC 9449 section 4.3) and returns the thumbprint of its key, or throws an Error with a `proof` message:
   * `method` and `urls` are what `htm` and `htu` must say, `accessToken` (a resource request) what `ath` must hash.
   */
  function checkProof(raw, { method, urls, accessToken }) {
    const refuse = (message) => {
      throw Object.assign(new Error(message), { proof: true });
    };
    if (typeof raw !== "string" || raw === "") refuse("the DPoP proof is missing");
    if (raw.includes(",")) refuse("more than one DPoP header");
    const parts = raw.split(".");
    if (parts.length !== 3 || parts.some((part) => part === "")) refuse("the DPoP proof is not a JWT");
    let head;
    let claims;
    try {
      head = JSON.parse(Buffer.from(parts[0], "base64url").toString("utf8"));
      claims = JSON.parse(Buffer.from(parts[1], "base64url").toString("utf8"));
    } catch {
      refuse("the DPoP proof is not made of JSON");
    }
    if (head === null || typeof head !== "object" || claims === null || typeof claims !== "object") refuse("the DPoP proof is not made of JSON objects");
    if (head.typ !== "dpop+jwt") refuse("typ must be dpop+jwt");
    if (head.alg !== "ES256") refuse("alg must be ES256 (the one this issuer takes)");
    const jwk = head.jwk;
    if (jwk === null || typeof jwk !== "object") refuse("the header has no jwk");
    for (const member of ["d", "p", "q", "dp", "dq", "qi", "k", "oth"]) {
      if (member in jwk) refuse(`the jwk holds a private member (${member})`);
    }
    if (jwk.kty !== "EC" || jwk.crv !== "P-256" || typeof jwk.x !== "string" || typeof jwk.y !== "string") refuse("jwk must be an EC P-256 public key");
    let key;
    try {
      key = createPublicKey({ key: { kty: "EC", crv: "P-256", x: jwk.x, y: jwk.y }, format: "jwk" });
    } catch {
      refuse("jwk is not a valid P-256 public key");
    }
    let signed = false;
    try {
      signed = verifySignature("sha256", Buffer.from(`${parts[0]}.${parts[1]}`), { key, dsaEncoding: "ieee-p1363" }, Buffer.from(parts[2], "base64url"));
    } catch {
      signed = false;
    }
    if (!signed) refuse("the signature does not verify with the jwk");
    if (typeof claims.jti !== "string" || claims.jti === "" || claims.jti.length > 256) refuse("jti is missing");
    if (claims.htm !== method) refuse(`htm must be ${method}`);
    let htu;
    try {
      htu = withoutQuery(String(claims.htu));
    } catch {
      refuse("htu is missing or not a URL");
    }
    if (!urls.includes(htu)) refuse(`htu must be ${urls[0]} (the URL of this request, with no query or fragment)`);
    if (typeof claims.iat !== "number" || !Number.isFinite(claims.iat)) refuse("iat is missing");
    const at = now();
    if (claims.iat < at - dpopLifetimeSecs - dpopSkewSecs) refuse("the proof is too old");
    if (claims.iat > at + dpopSkewSecs) refuse("the proof's iat is in the future");
    if (accessToken !== undefined && claims.ath !== b64url(createHash("sha256").update(accessToken).digest())) refuse("ath is not the hash of the access token");
    for (const [jti, until] of proofs) if (until <= at) proofs.delete(jti);
    if (proofs.has(claims.jti)) refuse("the proof was already used (jti)");
    if (proofs.size >= MAX_PROOFS) proofs.delete(proofs.keys().next().value);
    proofs.set(claims.jti, at + dpopLifetimeSecs + 2 * dpopSkewSecs);
    return { jkt: thumbprint(jwk) };
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
      ...(webClientId === ""
        ? {}
        : {
            revocation_endpoint: `${issuer}/revoke`,
            end_session_endpoint: `${publicUrl}/logout`,
            dpop_signing_alg_values_supported: ["ES256"],
            revocation_endpoint_auth_methods_supported: ["none", "client_secret_basic", "client_secret_post"],
          }),
      response_types_supported: ["code"],
      grant_types_supported: webClientId === "" ? ["authorization_code", "client_credentials"] : ["authorization_code", "client_credentials", "refresh_token"],
      subject_types_supported: ["public"],
      id_token_signing_alg_values_supported: ["RS256"],
      token_endpoint_auth_methods_supported: webClientId === "" ? ["client_secret_basic", "client_secret_post"] : ["client_secret_basic", "client_secret_post", "none"],
      code_challenge_methods_supported: ["S256", "plain"],
      scopes_supported: webClientId === "" ? ["openid", "email", "profile"] : WEB_SCOPES,
      claims_supported: ["sub", "iss", "aud", "exp", "iat", "email", "email_verified", "name", "preferred_username", rolesClaim],
    };
  }

  function authorize(req, res, url) {
    const q = url.searchParams;
    const redirectUri = q.get("redirect_uri") ?? "";
    const web = isWeb(q.get("client_id"));
    if (q.get("client_id") !== clientId && !web) {
      return reply(res, 400, `unknown client_id (this issuer has ${webClientId === "" ? `one: ${clientId}` : `two: ${clientId}, ${webClientId}`})`);
    }
    let target;
    try {
      target = new URL(redirectUri);
    } catch {
      return reply(res, 400, "redirect_uri is missing or not a URL");
    }
    if (target.protocol !== "http:" && target.protocol !== "https:") return reply(res, 400, "redirect_uri must be http or https");
    if (q.get("response_type") !== "code") return reply(res, 400, "response_type must be code");
    if (web) {
      if (webRedirectUris.length > 0 && !webRedirectUris.includes(redirectUri)) {
        return reply(res, 400, `redirect_uri ${JSON.stringify(redirectUri)} is not one of this client's (${webRedirectUris.join(", ")})`);
      }
      if (!q.get("code_challenge") || q.get("code_challenge_method") !== "S256") {
        return reply(res, 400, "this client is public: PKCE is required, code_challenge with code_challenge_method=S256");
      }
    }
    const hint = (q.get("login_hint") ?? "").trim().toLowerCase();
    const remembered = cookieOf(req, USER_COOKIE).trim().toLowerCase();
    const email = hint || (known(remembered) ? remembered : defaultUser);
    if (!known(email)) {
      return reply(res, 400, `login_hint names ${JSON.stringify(email)}, which is not a user of this issuer (users: ${Object.keys(table).join(", ")})`);
    }
    if (codes.size >= MAX_CODES) codes.delete(codes.keys().next().value);
    const code = b64url(randomBytes(24));
    const scopes = (q.get("scope") ?? "").split(/\s+/).filter(Boolean);
    codes.set(code, {
      client: web ? webClientId : clientId,
      scope: web ? (scopes.length === 0 ? ["openid", "email", "profile"] : scopes.filter((scope) => WEB_SCOPES.includes(scope))) : [],
      dpopJkt: web ? (q.get("dpop_jkt") ?? "") : "",
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
    if (isWeb(client.id)) return webTokenEndpoint(req, res, form);
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
      if (grantRecord.client !== clientId) return oauthError(res, 400, "invalid_grant", "the code was issued to another client");
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

  /** The tokens of a session of the public client: the access token is bound to the session's key, the refresh token is used once. */
  function issueWeb(session, nonce) {
    const at = now();
    const ttl = session.offline ? refreshOfflineSecs : refreshSessionSecs;
    const refresh = b64url(randomBytes(32));
    if (refreshTokens.size >= MAX_REFRESH_TOKENS) refreshTokens.delete(refreshTokens.keys().next().value);
    refreshTokens.set(refresh, { session, used: false, expires: at + ttl });
    return {
      access_token: webToken(session.email, { jkt: session.jkt }),
      id_token: webToken(session.email, { nonce }),
      refresh_token: refresh,
      token_type: "DPoP",
      expires_in: webTtlSecs,
      refresh_expires_in: session.offline ? 0 : ttl,
      scope: session.scope.join(" "),
    };
  }

  /** The proof of a request, or a 400 `invalid_dpop_proof` that has been sent (then `null`). */
  function proofOf(req, res, urls) {
    try {
      return checkProof(req.headers.dpop, { method: req.method, urls });
    } catch (error) {
      if (!error.proof) throw error;
      log(`invalid_dpop_proof: ${error.message}`);
      oauthError(res, 400, "invalid_dpop_proof", error.message);
      return null;
    }
  }

  function webTokenEndpoint(req, res, form) {
    const grant = form.get("grant_type");
    if (grant === "authorization_code") {
      const code = form.get("code") ?? "";
      const grantRecord = codes.get(code);
      codes.delete(code); // single use, whatever follows
      if (!grantRecord || grantRecord.expires < Date.now()) return oauthError(res, 400, "invalid_grant", "the code is unknown, used or expired");
      if (grantRecord.client !== webClientId) return oauthError(res, 400, "invalid_grant", "the code was issued to another client");
      if (form.has("redirect_uri") && form.get("redirect_uri") !== grantRecord.redirectUri) {
        return oauthError(res, 400, "invalid_grant", "redirect_uri is not the one of the authorize request");
      }
      const verifier = form.get("code_verifier") ?? "";
      if (!verifier || !same(b64url(createHash("sha256").update(verifier).digest()), grantRecord.challenge)) {
        return oauthError(res, 400, "invalid_grant", "code_verifier does not match code_challenge");
      }
      const proof = proofOf(req, res, tokenUrls);
      if (!proof) return undefined;
      if (grantRecord.dpopJkt && grantRecord.dpopJkt !== proof.jkt) {
        return oauthError(res, 400, "invalid_grant", "the proof's key is not the dpop_jkt of the authorize request");
      }
      const session = {
        client: webClientId,
        email: grantRecord.email,
        jkt: proof.jkt,
        scope: grantRecord.scope,
        offline: grantRecord.scope.includes("offline_access"),
        revoked: false,
      };
      log(`token: ${session.email}, authorization_code, ${session.offline ? "offline" : "session"} refresh token, key ${session.jkt.slice(0, 8)}`);
      return reply(res, 200, issueWeb(session, grantRecord.nonce));
    }
    if (grant === "refresh_token") {
      const record = refreshTokens.get(form.get("refresh_token") ?? "");
      if (!record || record.session.revoked || record.session.client !== webClientId) {
        return oauthError(res, 400, "invalid_grant", "the refresh token is unknown, or its session has ended");
      }
      if (record.expires <= now()) return oauthError(res, 400, "invalid_grant", "the refresh token has expired");
      // Bound to a key (as in Keycloak): no proof, or the proof of another key, is invalid_grant and uses nothing up.
      if (!req.headers.dpop) return oauthError(res, 400, "invalid_grant", "the refresh token is DPoP-bound: send a proof of its key");
      const proof = proofOf(req, res, tokenUrls);
      if (!proof) return undefined;
      if (proof.jkt !== record.session.jkt) return oauthError(res, 400, "invalid_grant", "the proof's key is not the key the refresh token is bound to");
      if (record.used) {
        record.session.revoked = true; // a refresh token is good once: a second use ends the session for both holders
        log(`refresh_token: reuse by ${record.session.email}, the session is revoked`);
        return oauthError(res, 400, "invalid_grant", "the refresh token was already used: the session is revoked");
      }
      record.used = true;
      log(`token: ${record.session.email}, refresh_token`);
      return reply(res, 200, issueWeb(record.session));
    }
    return oauthError(res, 400, grant === "client_credentials" ? "unauthorized_client" : "unsupported_grant_type", "this public client takes authorization_code and refresh_token only");
  }

  async function revokeEndpoint(req, res) {
    let form;
    try {
      form = new URLSearchParams(await readBody(req));
    } catch (error) {
      return oauthError(res, error.status === 413 ? 413 : 400, "invalid_request", "the body could not be read");
    }
    const client = clientOf(req, form);
    if (!isWeb(client.id) && !clientIsRight(client)) {
      return oauthError(res, 401, "invalid_client", "wrong client_id or client_secret", { "www-authenticate": 'Basic realm="mock-oidc"' });
    }
    const value = form.get("token") ?? "";
    if (value === "") return oauthError(res, 400, "invalid_request", "token is missing");
    const record = refreshTokens.get(value);
    // RFC 7009 section 2.2: an unknown token, or one of another client, is a 200 that does nothing. An access token is not tracked (its
    // `exp` ends it): a 200 too.
    if (record && record.session.client === client.id) {
      const proof = proofOf(req, res, revokeUrls);
      if (!proof) return undefined;
      if (proof.jkt !== record.session.jkt) return oauthError(res, 400, "invalid_dpop_proof", "the proof's key is not the key the token is bound to");
      record.session.revoked = true;
      log(`revoke: the session of ${record.session.email}`);
    }
    return reply(res, 200, undefined);
  }

  function logout(res, url) {
    const headers = { "set-cookie": `${USER_COOKIE}=; Path=/; Max-Age=0; SameSite=Lax` };
    const back = url.searchParams.get("post_logout_redirect_uri");
    if (!back) return reply(res, 200, "Signed out of the mock issuer. It keeps no session: an offline refresh token is still good (revoke it at /revoke).\n", headers);
    let target;
    try {
      target = new URL(back);
    } catch {
      return reply(res, 400, "post_logout_redirect_uri is not a URL");
    }
    if (target.protocol !== "http:" && target.protocol !== "https:") return reply(res, 400, "post_logout_redirect_uri must be http or https");
    if (url.searchParams.has("state")) target.searchParams.set("state", url.searchParams.get("state"));
    return reply(res, 302, undefined, { ...headers, location: target.toString() });
  }

  function userinfo(req, res) {
    const header = String(req.headers.authorization ?? "");
    const scheme = /^(bearer|dpop) /i.exec(header)?.[1].toLowerCase();
    const raw = scheme ? header.slice(header.indexOf(" ") + 1).trim() : "";
    const claims = raw ? keys.verify(raw) : null;
    if (!claims || claims.exp <= now()) return reply(res, 401, { error: "invalid_token" }, { "www-authenticate": 'Bearer error="invalid_token"' });
    const bound = claims.cnf?.jkt;
    if (bound && scheme !== "dpop") {
      return reply(res, 401, { error: "invalid_token", error_description: "a DPoP-bound token is sent with the DPoP scheme and a proof" }, { "www-authenticate": 'DPoP error="invalid_token", algs="ES256"' });
    }
    if (!bound && scheme === "dpop") return reply(res, 401, { error: "invalid_token", error_description: "this token is not DPoP-bound" }, { "www-authenticate": 'Bearer error="invalid_token"' });
    if (bound) {
      try {
        if (checkProof(req.headers.dpop, { method: "GET", urls: userinfoUrls, accessToken: raw }).jkt !== bound) throw Object.assign(new Error("the proof's key is not the token's"), { proof: true });
      } catch (error) {
        if (!error.proof) throw error;
        return reply(res, 401, { error: "invalid_dpop_proof", error_description: error.message }, { "www-authenticate": 'DPoP error="invalid_dpop_proof", algs="ES256"' });
      }
    }
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
    const clients = `clients: ${clientId} (confidential)${webClientId === "" ? "" : `, ${webClientId} (public, PKCE and DPoP)`}`;
    return reply(res, 200, `mock-oidc, issuer ${issuer}\n${clients}\ndefault user: ${defaultUser}\n${rows.join("\n")}\n`);
  }

  return createServer(async (req, res) => {
    try {
      const url = new URL(req.url, "http://localhost");
      const { pathname } = url;
      const get = req.method === "GET" || req.method === "HEAD";
      // CORS, for the origins listed and the endpoints a browser client calls (a preflight answers without reaching them).
      const origin = req.headers.origin;
      if (CORS_PATHS.has(pathname) && webClientId !== "") {
        const allowed = typeof origin === "string" && browserOrigins.includes(origin);
        if (req.method === "OPTIONS") {
          return reply(res, 204, undefined, {
            vary: "Origin",
            ...(allowed
              ? {
                  "access-control-allow-origin": origin,
                  "access-control-allow-methods": "GET, POST, OPTIONS",
                  "access-control-allow-headers": req.headers["access-control-request-headers"] ?? "content-type, dpop, authorization",
                  "access-control-max-age": "600",
                }
              : {}),
          });
        }
        res.setHeader("vary", "Origin");
        if (allowed) res.setHeader("access-control-allow-origin", origin);
      }
      if (pathname === "/.well-known/openid-configuration" && get) return reply(res, 200, discovery());
      if ((pathname === "/jwks" || pathname === "/.well-known/jwks.json") && get) return reply(res, 200, { keys: [keys.jwk] });
      if (pathname === "/authorize" && get) return authorize(req, res, url);
      if (pathname === "/token") {
        if (req.method !== "POST") return reply(res, 405, "POST", { allow: "POST" });
        return await tokenEndpoint(req, res);
      }
      if (pathname === "/userinfo" && get) return userinfo(req, res);
      if (pathname === "/revoke" && webClientId !== "") {
        if (req.method !== "POST") return reply(res, 405, "POST", { allow: "POST" });
        return await revokeEndpoint(req, res);
      }
      if (pathname === "/logout" && get && webClientId !== "") return logout(res, url);
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

/** A comma-separated environment value as a list of non-empty, trimmed words. */
const list = (value) => String(value ?? "").split(",").map((word) => word.trim()).filter(Boolean);

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
    webClientId: process.env.WEB_CLIENT_ID ?? "dev-web",
    webTtlSecs: Number(process.env.WEB_TOKEN_TTL_SECS ?? 300),
    webRedirectUris: list(process.env.WEB_REDIRECT_URIS),
    browserOrigins: list(process.env.BROWSER_ORIGINS),
    dpopLifetimeSecs: Number(process.env.DPOP_LIFETIME_SECS ?? 10),
    dpopSkewSecs: Number(process.env.DPOP_SKEW_SECS ?? 15),
    refreshSessionSecs: Number(process.env.REFRESH_SESSION_SECS ?? 1800),
    refreshOfflineSecs: Number(process.env.REFRESH_OFFLINE_SECS ?? 30 * 24 * 3600),
  });
  server.listen(port, "0.0.0.0", () => {
    const web = process.env.WEB_CLIENT_ID ?? "dev-web";
    console.log(
      `mock-oidc: issuer ${issuer} on :${port}, ${Object.keys(users.users).length} users, tokens last ${process.env.TOKEN_TTL_SECS ?? 3600} s` +
        (web === "" ? "" : `, public client ${web} (DPoP, tokens last ${process.env.WEB_TOKEN_TTL_SECS ?? 300} s, CORS for ${list(process.env.BROWSER_ORIGINS).join(" ") || "no origin"})`),
    );
  });
  for (const signal of ["SIGTERM", "SIGINT"]) {
    process.on(signal, () => {
      server.close(() => process.exit(0));
      server.closeAllConnections();
    });
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) main();
