import { createHash, createPublicKey, randomBytes, verify } from "node:crypto";
import type http from "node:http";
import { PROFILES, type ProfileName } from "./fixtures";

/*
 * The mock plays the issuer too, for browser mode (ADR 0054): OpenID Connect discovery, an
 * authorization endpoint that approves at once as the person a test chose, a token endpoint with
 * authorization_code + PKCE and refresh_token with rotation and reuse detection, tokens bound to the
 * client's DPoP key (`cnf.jkt`), revocation and end-session. It also is what the orchestrator is in
 * front of the API: `verifyApi` checks the request's DPoP the way `auth-jwt` does (scheme, `typ`,
 * ES256, the signature by the header's key, `htm`, `htu`, `iat`, a `jti` seen once, `ath`, and the
 * token's `cnf.jkt` the proof key's thumbprint), so a spec that gets data has proved its headers.
 *
 * It lives behind a switch (`MOCK_BROWSER_AUTH=1`, `createMockServer({ browserAuth })`): without it
 * the mock is the edge deployment it always was.
 *
 * State is per test, by the `mock-registry` cookie the authorize and the API requests carry; the
 * token endpoint has no cookie (the browser calls it cross-origin without credentials), so a code
 * and a token family remember their session.
 */

export const CLIENT_ID = "another-agentic-web";
export const SCOPE = "openid email profile offline_access";

export type BrowserAuthOptions = {
  /** The origin the browser reaches the mock at (the issuer is `<origin>/oidc`). */
  origin: string;
  /** The origins the web is served at: the redirect URIs, CORS and `htu` are theirs (`auth.dpop.publicOrigins`). */
  publicOrigins: string[];
};

type Family = { sid: string; profile: ProfileName; jkt: string; dead: boolean };
type Code = {
  sid: string;
  profile: ProfileName;
  challenge: string;
  redirectUri: string;
  scope: string;
};
type Access = { family: Family; jkt: string; exp: number };

type Stats = {
  authorizations: number;
  codeGrants: number;
  refreshGrants: number;
  reuses: number;
  revocations: number;
  endSessions: number;
  apiRequests: number;
  refused: Array<{ error: string; description: string }>;
  /** Requests to a public route that carried an Authorization or a DPoP header: there must be none. */
  publicWithCredentials: number;
};

type Settings = { lifetime: number; refreshDelayMs: number; loginAs: ProfileName };

export type Verified =
  | { ok: true; profile: ProfileName; sid: string }
  | { ok: false; error: "invalid_token" | "invalid_dpop_proof"; description: string };

const b64u = (bytes: Buffer | string) => Buffer.from(bytes).toString("base64url");
const fromB64u = (text: string) => Buffer.from(text, "base64url");
const json = (part: string | undefined): Record<string, unknown> =>
  JSON.parse(fromB64u(part ?? "").toString("utf8")) as Record<string, unknown>;

/** RFC 7638 thumbprint of an EC public JWK. */
const thumbprint = (jwk: Record<string, unknown>) =>
  createHash("sha256")
    .update(JSON.stringify({ crv: jwk.crv, kty: jwk.kty, x: jwk.x, y: jwk.y }))
    .digest("base64url");

const sessionCookie = (req: http.IncomingMessage) =>
  /(?:^|;\s*)mock-registry=([^;]+)/.exec(req.headers.cookie ?? "")?.[1] ?? "default";

async function readText(req: http.IncomingMessage): Promise<string> {
  const chunks: Buffer[] = [];
  for await (const chunk of req) chunks.push(chunk as Buffer);
  return Buffer.concat(chunks).toString("utf8");
}

export function createIssuer(options: BrowserAuthOptions) {
  const issuer = `${options.origin}/oidc`;
  const codes = new Map<string, Code>();
  const families = new Map<string, Family>();
  /** Refresh tokens: spent ones stay so that a reuse is seen. */
  const refreshTokens = new Map<string, { family: Family; spent: boolean }>();
  const accessTokens = new Map<string, Access>();
  const seenJti = new Map<string, number>();
  const settings = new Map<string, Settings>();
  const stats = new Map<string, Stats>();

  const settingsOf = (sid: string): Settings => {
    let s = settings.get(sid);
    if (!s) {
      s = { lifetime: 300, refreshDelayMs: 0, loginAs: "user" };
      settings.set(sid, s);
    }
    return s;
  };
  const statsOf = (sid: string): Stats => {
    let s = stats.get(sid);
    if (!s) {
      s = {
        authorizations: 0,
        codeGrants: 0,
        refreshGrants: 0,
        reuses: 0,
        revocations: 0,
        endSessions: 0,
        apiRequests: 0,
        refused: [],
        publicWithCredentials: 0,
      };
      stats.set(sid, s);
    }
    return s;
  };

  const allowedOrigin = (req: http.IncomingMessage): string | undefined => {
    const origin = req.headers.origin;
    return typeof origin === "string" && options.publicOrigins.includes(origin)
      ? origin
      : undefined;
  };
  const cors = (req: http.IncomingMessage, res: http.ServerResponse) => {
    const origin = allowedOrigin(req);
    if (!origin) return;
    res.setHeader("Access-Control-Allow-Origin", origin);
    res.setHeader("Vary", "Origin");
    res.setHeader("Access-Control-Allow-Headers", "DPoP, Content-Type, Authorization");
    res.setHeader("Access-Control-Allow-Methods", "GET, POST, OPTIONS");
    // `Date` is not exposed: the page must not depend on reading the issuer's clock
  };
  const send = (
    req: http.IncomingMessage,
    res: http.ServerResponse,
    status: number,
    body: unknown,
    headers: Record<string, string> = {},
  ) => {
    cors(req, res);
    res.writeHead(status, {
      "Content-Type": "application/json",
      "Cache-Control": "no-store",
      ...headers,
    });
    res.end(JSON.stringify(body));
  };
  const redirect = (res: http.ServerResponse, to: string) => {
    res.writeHead(302, { Location: to, "Cache-Control": "no-store" });
    res.end();
  };

  /** One proof, checked as the orchestrator and the issuer do. Throws a description on any fault. */
  function checkProof(
    proof: string | undefined,
    expect: { method: string; urls: string[]; token?: string },
    windowSeconds: { past: number; future: number },
  ): { jkt: string } {
    if (!proof) throw new Error("no DPoP proof");
    const [h, p, s] = proof.split(".");
    const header = json(h);
    if (header.typ !== "dpop+jwt") throw new Error("typ is not dpop+jwt");
    if (header.alg !== "ES256") throw new Error("alg is not ES256");
    const jwk = header.jwk as Record<string, unknown> | undefined;
    if (jwk?.kty !== "EC" || jwk.crv !== "P-256" || "d" in jwk) {
      throw new Error("jwk is not a public P-256 key");
    }
    const key = createPublicKey({ key: jwk as never, format: "jwk" });
    const good = verify(
      "sha256",
      Buffer.from(`${h}.${p}`),
      { key, dsaEncoding: "ieee-p1363" },
      fromB64u(s ?? ""),
    );
    if (!good) throw new Error("the signature is not the key's");
    const claims = json(p);
    if (claims.htm !== expect.method) throw new Error(`htm is ${String(claims.htm)}`);
    if (typeof claims.htu !== "string" || !expect.urls.includes(claims.htu)) {
      throw new Error(`htu ${String(claims.htu)} is not one of ${expect.urls.join(", ")}`);
    }
    const now = Math.floor(Date.now() / 1000);
    if (
      typeof claims.iat !== "number" ||
      claims.iat < now - windowSeconds.past ||
      claims.iat > now + windowSeconds.future
    ) {
      throw new Error(`iat ${String(claims.iat)} is out of the window (now ${now})`);
    }
    if (typeof claims.jti !== "string" || !claims.jti) throw new Error("no jti");
    const seen = seenJti.get(claims.jti);
    if (seen !== undefined) throw new Error("jti was used before");
    seenJti.set(claims.jti, Date.now());
    for (const [jti, at] of seenJti) if (Date.now() - at > 120_000) seenJti.delete(jti);
    if (expect.token !== undefined) {
      const ath = createHash("sha256").update(expect.token).digest("base64url");
      if (claims.ath !== ath) throw new Error("ath is not the token's hash");
    }
    return { jkt: thumbprint(jwk) };
  }

  const unsigned = (payload: Record<string, unknown>, alg = "ES256") =>
    `${b64u(JSON.stringify({ alg, typ: "JWT" }))}.${b64u(JSON.stringify(payload))}.${b64u(randomBytes(16))}`;

  function mint(family: Family) {
    const { lifetime } = settingsOf(family.sid);
    const user = PROFILES[family.profile];
    const now = Math.floor(Date.now() / 1000);
    const claims = { iss: issuer, sub: `sub-${user.user}`, email: user.email ?? user.user };
    const accessToken = unsigned({
      ...claims,
      aud: "another-agentic",
      exp: now + lifetime,
      iat: now,
      cnf: { jkt: family.jkt },
      jti: b64u(randomBytes(9)),
    });
    accessTokens.set(accessToken, { family, jkt: family.jkt, exp: now + lifetime });
    const refreshToken = `rt-${b64u(randomBytes(24))}`;
    refreshTokens.set(refreshToken, { family, spent: false });
    const idToken = unsigned({ ...claims, aud: CLIENT_ID, exp: now + 300, iat: now }, "RS256");
    return {
      access_token: accessToken,
      token_type: "DPoP",
      expires_in: lifetime,
      refresh_token: refreshToken,
      id_token: idToken,
      scope: SCOPE,
    };
  }

  const metadata = () => ({
    issuer,
    authorization_endpoint: `${issuer}/auth`,
    token_endpoint: `${issuer}/token`,
    revocation_endpoint: `${issuer}/revoke`,
    end_session_endpoint: `${issuer}/logout`,
    jwks_uri: `${issuer}/certs`,
    response_types_supported: ["code"],
    subject_types_supported: ["public"],
    id_token_signing_alg_values_supported: ["RS256"],
    code_challenge_methods_supported: ["S256"],
    dpop_signing_alg_values_supported: ["ES256"],
    authorization_response_iss_parameter_supported: true,
    grant_types_supported: ["authorization_code", "refresh_token"],
  });

  async function token(req: http.IncomingMessage, res: http.ServerResponse) {
    const form = new URLSearchParams(await readText(req));
    const grant = form.get("grant_type");
    let jkt: string;
    try {
      jkt = checkProof(
        req.headers.dpop as string | undefined,
        { method: "POST", urls: [`${issuer}/token`] },
        { past: 25, future: 15 },
      ).jkt;
    } catch (e) {
      return send(req, res, 400, { error: "invalid_dpop_proof", error_description: String(e) });
    }
    if (grant === "authorization_code") {
      const code = codes.get(form.get("code") ?? "");
      codes.delete(form.get("code") ?? "");
      if (!code)
        return send(req, res, 400, { error: "invalid_grant", error_description: "bad code" });
      const challenge = createHash("sha256")
        .update(form.get("code_verifier") ?? "")
        .digest("base64url");
      if (challenge !== code.challenge || form.get("redirect_uri") !== code.redirectUri) {
        return send(req, res, 400, {
          error: "invalid_grant",
          error_description: "PKCE or redirect",
        });
      }
      statsOf(code.sid).codeGrants += 1;
      const family: Family = { sid: code.sid, profile: code.profile, jkt, dead: false };
      families.set(`${code.sid}:${families.size}`, family);
      return send(req, res, 200, mint(family));
    }
    if (grant === "refresh_token") {
      const entry = refreshTokens.get(form.get("refresh_token") ?? "");
      if (!entry) return send(req, res, 400, { error: "invalid_grant" });
      const { family } = entry;
      const wait = settingsOf(family.sid).refreshDelayMs;
      if (wait > 0) await new Promise((r) => setTimeout(r, wait));
      statsOf(family.sid).refreshGrants += 1;
      if (entry.spent) {
        // a refresh token used twice: the session is over for whoever holds it (RFC 9700 section 4.14)
        family.dead = true;
        statsOf(family.sid).reuses += 1;
        return send(req, res, 400, {
          error: "invalid_grant",
          error_description: "Token reuse detected",
        });
      }
      if (family.dead || family.jkt !== jkt) return send(req, res, 400, { error: "invalid_grant" });
      entry.spent = true;
      return send(req, res, 200, mint(family));
    }
    return send(req, res, 400, { error: "unsupported_grant_type" });
  }

  /** Every route under `/oidc/`; false for anything that is not the issuer's. */
  async function handle(
    req: http.IncomingMessage,
    res: http.ServerResponse,
    url: URL,
  ): Promise<boolean> {
    const path = url.pathname;
    if (!path.startsWith("/oidc/")) return false;
    const method = req.method ?? "GET";
    if (method === "OPTIONS") {
      cors(req, res);
      res.writeHead(204).end();
      return true;
    }
    if (path === "/oidc/.well-known/openid-configuration") {
      send(req, res, 200, metadata());
      return true;
    }
    if (path === "/oidc/auth" && method === "GET") {
      const sid = sessionCookie(req);
      const redirectUri = url.searchParams.get("redirect_uri") ?? "";
      const state = url.searchParams.get("state") ?? "";
      const allowed = options.publicOrigins.map((o) => `${o}/auth/callback`);
      if (url.searchParams.get("client_id") !== CLIENT_ID || !allowed.includes(redirectUri)) {
        send(req, res, 400, {
          error: "invalid_request",
          error_description: "client or redirect_uri",
        });
        return true;
      }
      if (
        url.searchParams.get("response_type") !== "code" ||
        url.searchParams.get("code_challenge_method") !== "S256" ||
        !url.searchParams.get("code_challenge") ||
        !state
      ) {
        send(req, res, 400, {
          error: "invalid_request",
          error_description: "PKCE S256 and a state",
        });
        return true;
      }
      const code = `code-${b64u(randomBytes(18))}`;
      const scope = url.searchParams.get("scope") ?? "";
      codes.set(code, {
        sid,
        profile: settingsOf(sid).loginAs,
        challenge: url.searchParams.get("code_challenge") as string,
        redirectUri,
        scope,
      });
      statsOf(sid).authorizations += 1;
      const back = new URL(redirectUri);
      back.searchParams.set("code", code);
      back.searchParams.set("state", state);
      back.searchParams.set("iss", issuer);
      redirect(res, back.href);
      return true;
    }
    if (path === "/oidc/token" && method === "POST") {
      await token(req, res);
      return true;
    }
    if (path === "/oidc/revoke" && method === "POST") {
      const form = new URLSearchParams(await readText(req));
      const entry = refreshTokens.get(form.get("token") ?? "");
      try {
        checkProof(
          req.headers.dpop as string | undefined,
          { method: "POST", urls: [`${issuer}/revoke`] },
          { past: 25, future: 15 },
        );
      } catch (e) {
        send(req, res, 400, { error: "invalid_dpop_proof", error_description: String(e) });
        return true;
      }
      if (entry) {
        entry.family.dead = true;
        statsOf(entry.family.sid).revocations += 1;
      }
      cors(req, res);
      res.writeHead(200).end();
      return true;
    }
    if (path === "/oidc/logout" && method === "GET") {
      const sid = sessionCookie(req);
      statsOf(sid).endSessions += 1;
      const to = url.searchParams.get("post_logout_redirect_uri") ?? "";
      const allowed = options.publicOrigins.map((o) => `${o}/`);
      if (url.searchParams.get("client_id") !== CLIENT_ID || !allowed.includes(to)) {
        send(req, res, 400, {
          error: "invalid_request",
          error_description: "post_logout_redirect_uri",
        });
        return true;
      }
      redirect(res, to);
      return true;
    }
    send(req, res, 404, { error: "not_found" });
    return true;
  }

  /** The test hooks: what a test sets for its session, and what the issuer and the API saw. */
  function hooks(
    req: http.IncomingMessage,
    res: http.ServerResponse,
    url: URL,
    sid: string,
  ): boolean {
    const path = url.pathname;
    const method = req.method ?? "GET";
    if (path === "/__mock/issuer-config" && method === "POST") {
      const s = settingsOf(sid);
      const lifetime = url.searchParams.get("lifetime");
      const delay = url.searchParams.get("refreshDelay");
      const login = url.searchParams.get("loginAs");
      if (lifetime !== null) s.lifetime = Number(lifetime);
      if (delay !== null) s.refreshDelayMs = Number(delay);
      if (login !== null) {
        if (!Object.hasOwn(PROFILES, login)) {
          send(req, res, 400, { error: "loginAs is a profile of fixtures.ts" });
          return true;
        }
        s.loginAs = login as ProfileName;
      }
      res.writeHead(204).end();
      return true;
    }
    if (path === "/__mock/issuer-revoke" && method === "POST") {
      // an administrator's revocation: every refresh token of this session is refused from now on
      for (const family of families.values()) if (family.sid === sid) family.dead = true;
      res.writeHead(204).end();
      return true;
    }
    if (path === "/__mock/issuer" && method === "GET") {
      send(req, res, 200, { ...statsOf(sid), settings: settingsOf(sid) });
      return true;
    }
    return false;
  }

  /** What the orchestrator's `auth-jwt` does with `Authorization` and `DPoP`. */
  function verifyApi(req: http.IncomingMessage, url: URL): Verified {
    const sid = sessionCookie(req);
    const fail = (error: "invalid_token" | "invalid_dpop_proof", description: string): Verified => {
      statsOf(sid).refused.push({ error, description });
      return { ok: false, error, description };
    };
    const authorization = req.headers.authorization ?? "";
    const [scheme, token] = authorization.split(" ");
    if (!token) return fail("invalid_token", "no credentials");
    const known = accessTokens.get(token);
    if (scheme !== "DPoP") {
      return fail("invalid_token", "a token with cnf is not a Bearer token (RFC 9449 section 7.1)");
    }
    if (!known) return fail("invalid_token", "unknown token");
    if (known.exp < Math.floor(Date.now() / 1000))
      return fail("invalid_token", "the token has expired");
    if (known.family.dead) return fail("invalid_token", "the session is over");
    let jkt: string;
    try {
      jkt = checkProof(
        req.headers.dpop as string | undefined,
        {
          method: req.method ?? "GET",
          urls: options.publicOrigins.map((o) => `${o}${url.pathname}`),
          token,
        },
        { past: 60, future: 5 },
      ).jkt;
    } catch (e) {
      return fail("invalid_dpop_proof", String(e));
    }
    if (jkt !== known.jkt)
      return fail("invalid_token", "the proof's key is not the token's (cnf.jkt)");
    statsOf(sid).apiRequests += 1;
    return { ok: true, profile: known.family.profile, sid };
  }

  return {
    issuer,
    handle,
    hooks,
    verifyApi,
    /** A request to a public route: it must carry no credentials. */
    notePublic(req: http.IncomingMessage) {
      if (req.headers.authorization || req.headers.dpop) {
        statsOf(sessionCookie(req)).publicWithCredentials += 1;
      }
    },
    publicConfig: { issuer, clientId: CLIENT_ID, scope: SCOPE },
  };
}

export type Issuer = ReturnType<typeof createIssuer>;
