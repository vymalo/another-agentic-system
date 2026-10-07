import { accessTokenHash, base64url, htuOf } from "./dpop";

/*
 * A stand-in for the issuer, as a `fetch`, for the auth module's tests: discovery, the token
 * endpoint (authorization code with PKCE, and refresh with rotation and reuse detection, both
 * bound to the proof's key) and revocation. It checks the proofs it is sent, so a test that gets
 * a token has proved the proof right.
 */

export const ISSUER = "https://issuer.test/realms/demo";
export const CLIENT_ID = "another-agentic-web";
export const ORIGIN = "https://app.test";

const encoder = new TextEncoder();
const decoder = new TextDecoder();

const fromB64url = (text: string): Uint8Array<ArrayBuffer> =>
  Uint8Array.from(atob(text.replaceAll("-", "+").replaceAll("_", "/")), (c) => c.charCodeAt(0));
const jsonOf = (part: string | undefined): Record<string, unknown> =>
  JSON.parse(decoder.decode(fromB64url(part ?? ""))) as Record<string, unknown>;
const unsigned = (header: unknown, payload: unknown) =>
  `${base64url(encoder.encode(JSON.stringify(header)))}.${base64url(encoder.encode(JSON.stringify(payload)))}.sig`;

export async function thumbprint(jwk: JsonWebKey): Promise<string> {
  const canonical = JSON.stringify({ crv: jwk.crv, kty: jwk.kty, x: jwk.x, y: jwk.y });
  return base64url(await crypto.subtle.digest("SHA-256", encoder.encode(canonical)));
}

/** Verifies a DPoP proof's signature and claims; returns the claims and the key's thumbprint. */
export async function verifyProof(
  proof: string,
  expect: { method: string; url: string; accessToken?: string },
  nowSeconds: number,
): Promise<{ claims: Record<string, unknown>; jkt: string }> {
  const [h, p, s] = proof.split(".");
  const header = jsonOf(h);
  if (header.typ !== "dpop+jwt" || header.alg !== "ES256") throw new Error("bad typ or alg");
  const jwk = header.jwk as JsonWebKey;
  const key = await crypto.subtle.importKey(
    "jwk",
    jwk,
    { name: "ECDSA", namedCurve: "P-256" },
    false,
    ["verify"],
  );
  const ok = await crypto.subtle.verify(
    { name: "ECDSA", hash: "SHA-256" },
    key,
    fromB64url(s ?? ""),
    encoder.encode(`${h}.${p}`),
  );
  if (!ok) throw new Error("bad signature");
  const claims = jsonOf(p);
  if (claims.htm !== expect.method) throw new Error(`bad htm ${String(claims.htm)}`);
  if (claims.htu !== htuOf(expect.url)) throw new Error(`bad htu ${String(claims.htu)}`);
  if (typeof claims.jti !== "string") throw new Error("no jti");
  if (typeof claims.iat !== "number" || Math.abs(claims.iat - nowSeconds) > 10) {
    throw new Error(`bad iat ${String(claims.iat)}`);
  }
  if (expect.accessToken !== undefined) {
    if (claims.ath !== (await accessTokenHash(expect.accessToken))) throw new Error("bad ath");
  }
  return { claims, jkt: await thumbprint(jwk) };
}

type Grant = { jkt: string; user: User; family: number };
export type User = { sub: string; email: string };

export type FakeIssuer = {
  fetch: typeof fetch;
  calls: { url: string; grant?: string }[];
  /** Refresh tokens redeemed with `grant_type=refresh_token`. */
  grants: () => number;
  /** The next code the authorization endpoint would hand out, for `user`. */
  approve: (user: User, challenge: string) => string;
  revoked: string[];
  /** Seconds the issuer's clock is ahead of the test's. */
  skew: number;
  /** Makes every refresh answer 503. */
  down: boolean;
  /** The access-token lifetime in seconds. */
  lifetime: number;
  /** Refuse the refresh token now: as an administrator's revocation would. */
  revokeAll: () => void;
};

export function fakeIssuer(): FakeIssuer {
  const codes = new Map<string, { user: User; challenge: string }>();
  const live = new Map<string, Grant>();
  const spent = new Set<string>();
  const dead = new Set<number>();
  let counter = 0;
  let families = 0;
  const calls: FakeIssuer["calls"] = [];
  const revoked: string[] = [];
  let refreshGrants = 0;

  const issuer: FakeIssuer = {
    calls,
    revoked,
    skew: 0,
    down: false,
    lifetime: 300,
    grants: () => refreshGrants,
    revokeAll() {
      for (const g of live.values()) dead.add(g.family);
    },
    approve(user, challenge) {
      const code = `code-${++counter}`;
      codes.set(code, { user, challenge });
      return code;
    },
    fetch: async (input, init) => {
      const request = new Request(input as RequestInfo, init);
      const url = new URL(request.url);
      const headers = { Date: new Date(Date.now() + issuer.skew * 1000).toUTCString() };
      const json = (status: number, body: unknown) =>
        new Response(JSON.stringify(body), {
          status,
          headers: { ...headers, "Content-Type": "application/json" },
        });
      calls.push({ url: request.url });
      if (url.pathname.endsWith("/.well-known/openid-configuration")) {
        return json(200, {
          issuer: ISSUER,
          authorization_endpoint: `${ISSUER}/auth`,
          token_endpoint: `${ISSUER}/token`,
          revocation_endpoint: `${ISSUER}/revoke`,
          end_session_endpoint: `${ISSUER}/logout`,
          jwks_uri: `${ISSUER}/certs`,
          dpop_signing_alg_values_supported: ["ES256"],
          code_challenge_methods_supported: ["S256"],
          id_token_signing_alg_values_supported: ["RS256"],
        });
      }
      const now = Math.floor((Date.now() + issuer.skew * 1000) / 1000);
      const dpop = request.headers.get("DPoP");
      if (url.pathname.endsWith("/revoke")) {
        const form = new URLSearchParams(await request.text());
        if (!dpop) return json(400, { error: "invalid_dpop_proof" });
        await verifyProof(dpop, { method: "POST", url: request.url }, now);
        revoked.push(form.get("token") ?? "");
        const grant = live.get(form.get("token") ?? "");
        if (grant) dead.add(grant.family);
        return new Response(null, { status: 200, headers });
      }
      if (!url.pathname.endsWith("/token")) return json(404, { error: "not_found" });
      const form = new URLSearchParams(await request.text());
      const type = form.get("grant_type") ?? "";
      calls[calls.length - 1] = { url: request.url, grant: type };
      if (!dpop) return json(400, { error: "invalid_dpop_proof" });
      let jkt: string;
      try {
        jkt = (await verifyProof(dpop, { method: "POST", url: request.url }, now)).jkt;
      } catch (e) {
        return json(400, { error: "invalid_dpop_proof", error_description: String(e) });
      }
      let user: User;
      let family: number;
      if (type === "authorization_code") {
        const issued = codes.get(form.get("code") ?? "");
        codes.delete(form.get("code") ?? "");
        if (!issued) return json(400, { error: "invalid_grant" });
        const challenge = base64url(
          await crypto.subtle.digest("SHA-256", encoder.encode(form.get("code_verifier") ?? "")),
        );
        if (challenge !== issued.challenge) return json(400, { error: "invalid_grant" });
        user = issued.user;
        family = ++families;
      } else if (type === "refresh_token") {
        if (issuer.down) return json(503, { error: "temporarily_unavailable" });
        refreshGrants++;
        const token = form.get("refresh_token") ?? "";
        if (spent.has(token)) {
          // reuse: the whole family ends
          const reused = live.get(token);
          if (reused) dead.add(reused.family);
          return json(400, { error: "invalid_grant", error_description: "Token reuse detected" });
        }
        const grant = live.get(token);
        if (!grant || dead.has(grant.family) || grant.jkt !== jkt) {
          return json(400, { error: "invalid_grant" });
        }
        spent.add(token);
        user = grant.user;
        family = grant.family;
      } else {
        return json(400, { error: "unsupported_grant_type" });
      }
      const refresh = `refresh-${++counter}`;
      live.set(refresh, { jkt, user, family });
      const access = unsigned(
        { alg: "ES256", typ: "at+jwt" },
        {
          iss: ISSUER,
          sub: user.sub,
          email: user.email,
          aud: "another-agentic",
          exp: now + issuer.lifetime,
          iat: now,
          cnf: { jkt },
          jti: `at-${++counter}`,
        },
      );
      const idToken = unsigned(
        { alg: "RS256", typ: "JWT" },
        {
          iss: ISSUER,
          sub: user.sub,
          email: user.email,
          aud: CLIENT_ID,
          exp: now + 300,
          iat: now,
        },
      );
      return json(200, {
        access_token: access,
        token_type: "DPoP",
        expires_in: issuer.lifetime,
        refresh_token: refresh,
        id_token: idToken,
        scope: "openid email profile offline_access",
      });
    },
  };
  return issuer;
}
