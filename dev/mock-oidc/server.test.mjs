// node --test dev/mock-oidc/server.test.mjs
// Holds the mock issuer to what the orchestrator (docs/api/config.md, "Authentication") and oauth2-proxy read of an issuer:
// discovery, the key set, the code flow with PKCE, client_credentials with a user, and the claims of the token; and, for the browser web
// (ADR 0054), the public client: PKCE required, DPoP-bound tokens, a refresh token good once (reuse ends the session), revocation, CORS.
import assert from "node:assert/strict";
import { createHash, createPublicKey, createVerify } from "node:crypto";
import { readFileSync } from "node:fs";
import { after, before, describe, it } from "node:test";
import { call, claimsOf, makeProof, newKey, pkce } from "../browser-auth/flow.mjs";
import { createMockServer, subjectOf, thumbprint } from "./server.mjs";

const users = JSON.parse(readFileSync(new URL("./users.json", import.meta.url), "utf8"));
const ISSUER = "http://mock-oidc:8080";
let clock = 1_800_000_000;
let server;
let base;

before(async () => {
  server = createMockServer({ users, issuer: ISSUER, publicUrl: "http://127.0.0.1:8099", now: () => clock });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  base = `http://127.0.0.1:${server.address().port}`;
});
after(() => {
  server.close();
  server.closeAllConnections();
});

const form = (fields) => new URLSearchParams(fields).toString();
const post = (path, body, headers = {}) =>
  fetch(base + path, { method: "POST", headers: { "content-type": "application/x-www-form-urlencoded", ...headers }, body: form(body) });
const creds = { client_id: "dev-chat", client_secret: "dev-client-secret" };

/** The claims of a token whose RS256 signature the JWKS verifies, else a failed assertion. */
async function verified(token) {
  const { keys } = await (await fetch(`${base}/jwks`)).json();
  const [head, body, signature] = token.split(".");
  const header = JSON.parse(Buffer.from(head, "base64url").toString());
  assert.equal(header.alg, "RS256");
  assert.equal(header.typ, "JWT");
  const jwk = keys.find((k) => k.kid === header.kid);
  assert.ok(jwk, "the kid of the token is in the JWKS");
  const key = createPublicKey({ key: jwk, format: "jwk" });
  assert.ok(createVerify("RSA-SHA256").update(`${head}.${body}`).verify(key, Buffer.from(signature, "base64url")), "signature");
  return JSON.parse(Buffer.from(body, "base64url").toString());
}

describe("discovery and keys", () => {
  it("names the issuer and the endpoints, the browser's one on the public address", async () => {
    const doc = await (await fetch(`${base}/.well-known/openid-configuration`)).json();
    assert.equal(doc.issuer, ISSUER);
    assert.equal(doc.jwks_uri, `${ISSUER}/jwks`);
    assert.equal(doc.token_endpoint, `${ISSUER}/token`);
    assert.equal(doc.authorization_endpoint, "http://127.0.0.1:8099/authorize");
    assert.deepEqual(doc.id_token_signing_alg_values_supported, ["RS256"]);
  });

  it("serves one RSA key of at least 2048 bits, with a kid, at both paths", async () => {
    for (const path of ["/jwks", "/.well-known/jwks.json"]) {
      const { keys } = await (await fetch(base + path)).json();
      assert.equal(keys.length, 1);
      assert.equal(keys[0].kty, "RSA");
      assert.equal(keys[0].alg, "RS256");
      assert.ok(keys[0].kid);
      assert.ok(Buffer.from(keys[0].n, "base64url").length >= 256);
    }
  });
});

describe("client_credentials", () => {
  it("gives a user's token with the claims the orchestrator reads", async () => {
    const res = await post("/token", { grant_type: "client_credentials", user: "admin@example.com", ...creds });
    assert.equal(res.status, 200);
    assert.equal(res.headers.get("cache-control"), "no-store");
    const body = await res.json();
    assert.equal(body.token_type, "Bearer");
    assert.equal(body.expires_in, 3600);
    const claims = await verified(body.access_token);
    assert.equal(claims.iss, ISSUER);
    assert.equal(claims.aud, "dev-chat");
    assert.equal(claims.email, "admin@example.com");
    assert.equal(claims.email_verified, true);
    assert.deepEqual(claims.roles, ["admin"]);
    assert.equal(claims.iat, clock);
    assert.equal(claims.exp, clock + 3600);
    assert.equal(claims.sub, subjectOf("admin@example.com"));
    assert.notEqual(claims.sub, claims.email);
  });

  it("takes the default user when none is named, and the audience asked for", async () => {
    const claims = await verified((await (await post("/token", { grant_type: "client_credentials", audience: "someone-else", ...creds })).json()).access_token);
    assert.equal(claims.email, "dev@example.com");
    assert.deepEqual(claims.roles, ["user"]);
    assert.equal(claims.aud, "someone-else");
  });

  it("gives a user with no role an empty roles claim, and every user of users.json a token", async () => {
    for (const [email, user] of Object.entries(users.users)) {
      const claims = await verified((await (await post("/token", { grant_type: "client_credentials", user: email, ...creds })).json()).access_token);
      assert.deepEqual(claims.roles, user.roles, email);
    }
  });

  it("refuses a user it does not know, and a client it does not know", async () => {
    const stranger = await post("/token", { grant_type: "client_credentials", user: "nobody@example.com", ...creds });
    assert.equal(stranger.status, 400);
    assert.equal((await stranger.json()).error, "invalid_request");
    const wrong = await post("/token", { grant_type: "client_credentials", client_id: "dev-chat", client_secret: "nope" });
    assert.equal(wrong.status, 401);
    assert.equal((await wrong.json()).error, "invalid_client");
  });

  it("reads the client from HTTP Basic too", async () => {
    const basic = `Basic ${Buffer.from("dev-chat:dev-client-secret").toString("base64")}`;
    const res = await post("/token", { grant_type: "client_credentials", user: "dev@example.com" }, { authorization: basic });
    assert.equal(res.status, 200);
  });

  it("refuses a grant it does not offer", async () => {
    const res = await post("/token", { grant_type: "password", ...creds });
    assert.equal(res.status, 400);
    assert.equal((await res.json()).error, "unsupported_grant_type");
  });
});

describe("the authorization code flow", () => {
  const redirect = "http://127.0.0.1:8080/oauth2/callback";
  const authorizeUrl = (extra = {}) =>
    `${base}/authorize?${form({ client_id: "dev-chat", redirect_uri: redirect, response_type: "code", state: "s1", nonce: "n1", ...extra })}`;

  it("approves the login_hint, and the default user without one, and answers with the state", async () => {
    for (const [hint, email] of [["admin@example.com", "admin@example.com"], [undefined, "dev@example.com"]]) {
      const res = await fetch(authorizeUrl(hint ? { login_hint: hint } : {}), { redirect: "manual" });
      assert.equal(res.status, 302);
      const location = new URL(res.headers.get("location"));
      assert.equal(`${location.origin}${location.pathname}`, redirect);
      assert.equal(location.searchParams.get("state"), "s1");
      const code = location.searchParams.get("code");
      const body = await (await post("/token", { grant_type: "authorization_code", code, redirect_uri: redirect, ...creds })).json();
      const id = await verified(body.id_token);
      assert.equal(id.email, email);
      assert.equal(id.aud, "dev-chat");
      assert.equal(id.nonce, "n1");
      assert.equal((await verified(body.access_token)).email, email);
    }
  });

  it("uses the user /login-as remembered, and a login_hint beats it", async () => {
    const set = await fetch(`${base}/login-as?user=chat-only@example.com`);
    assert.equal(set.status, 200);
    const cookie = set.headers.get("set-cookie").split(";")[0];
    const redeem = async (extra) => {
      const res = await fetch(authorizeUrl(extra), { redirect: "manual", headers: { cookie } });
      const code = new URL(res.headers.get("location")).searchParams.get("code");
      return verified((await (await post("/token", { grant_type: "authorization_code", code, ...creds })).json()).id_token);
    };
    assert.equal((await redeem({})).email, "chat-only@example.com");
    assert.equal((await redeem({ login_hint: "admin@example.com" })).email, "admin@example.com");
    assert.equal((await fetch(`${base}/login-as?user=nobody@example.com`)).status, 400);
  });

  it("gives a code once", async () => {
    const res = await fetch(authorizeUrl(), { redirect: "manual" });
    const code = new URL(res.headers.get("location")).searchParams.get("code");
    assert.equal((await post("/token", { grant_type: "authorization_code", code, ...creds })).status, 200);
    const again = await post("/token", { grant_type: "authorization_code", code, ...creds });
    assert.equal(again.status, 400);
    assert.equal((await again.json()).error, "invalid_grant");
  });

  it("checks PKCE: the right verifier redeems, a wrong one does not", async () => {
    const verifier = "a-code-verifier-of-at-least-43-characters-0123456789";
    const challenge = createHash("sha256").update(verifier).digest("base64url");
    const start = async () => {
      const res = await fetch(authorizeUrl({ code_challenge: challenge, code_challenge_method: "S256" }), { redirect: "manual" });
      return new URL(res.headers.get("location")).searchParams.get("code");
    };
    assert.equal((await post("/token", { grant_type: "authorization_code", code: await start(), code_verifier: verifier, ...creds })).status, 200);
    assert.equal((await post("/token", { grant_type: "authorization_code", code: await start(), code_verifier: "wrong", ...creds })).status, 400);
    assert.equal((await post("/token", { grant_type: "authorization_code", code: await start(), ...creds })).status, 400);
  });

  it("refuses an unknown client, a missing redirect_uri and a user it does not know", async () => {
    assert.equal((await fetch(authorizeUrl({ client_id: "other" }), { redirect: "manual" })).status, 400);
    assert.equal((await fetch(`${base}/authorize?client_id=dev-chat&response_type=code`, { redirect: "manual" })).status, 400);
    assert.equal((await fetch(authorizeUrl({ login_hint: "nobody@example.com" }), { redirect: "manual" })).status, 400);
  });
});

describe("userinfo and the rest", () => {
  it("answers the claims of a good access token and 401 to anything else", async () => {
    const { access_token: access } = await (await post("/token", { grant_type: "client_credentials", user: "admin@example.com", ...creds })).json();
    const ok = await fetch(`${base}/userinfo`, { headers: { authorization: `Bearer ${access}` } });
    assert.equal(ok.status, 200);
    const info = await ok.json();
    assert.equal(info.email, "admin@example.com");
    assert.deepEqual(info.roles, ["admin"]);
    assert.equal((await fetch(`${base}/userinfo`, { headers: { authorization: `Bearer ${access}x` } })).status, 401);
    assert.equal((await fetch(`${base}/userinfo`)).status, 401);
    clock += 7200; // an expired token is refused
    assert.equal((await fetch(`${base}/userinfo`, { headers: { authorization: `Bearer ${access}` } })).status, 401);
  });

  it("answers /healthz and 404 elsewhere", async () => {
    assert.equal((await fetch(`${base}/healthz`)).status, 200);
    assert.equal((await fetch(`${base}/nothing`)).status, 404);
    assert.equal((await fetch(`${base}/token`)).status, 405);
  });
});

// ---- the public client of the browser web (ADR 0054) ------------------------------------------------------------------------------
describe("the public client", () => {
  const WEB = "dev-web";
  const redirect = "http://edge:8080/auth/callback";
  const origin = "http://edge:8080";
  let web;
  let url;
  let time = 1_900_000_000;
  const tokenUrl = () => `http://mock-oidc:8080/token`;

  before(async () => {
    web = createMockServer({
      users,
      issuer: ISSUER,
      publicUrl: "http://127.0.0.1:8099",
      now: () => time,
      webRedirectUris: [redirect],
      browserOrigins: [origin],
    });
    await new Promise((resolve) => web.listen(0, "127.0.0.1", resolve));
    url = `http://127.0.0.1:${web.address().port}`;
  });
  after(() => {
    web.close();
    web.closeAllConnections();
  });

  const fields = (values) => new URLSearchParams(values).toString();
  const FORM = { "content-type": "application/x-www-form-urlencoded" };
  const proofAt = (key, extra = {}) =>
    makeProof(key, { method: "POST", url: tokenUrl(), ...extra, claims: { iat: time, ...(extra.claims ?? {}) } });

  /** Authorize as `email` with PKCE and return what the redirect carried. */
  async function authorize(email, extra = {}, scope = "openid email profile offline_access") {
    const pair = pkce();
    const query = fields({
      client_id: WEB, redirect_uri: redirect, response_type: "code", scope, state: "s1", nonce: "n1", login_hint: email,
      code_challenge: pair.challenge, code_challenge_method: "S256", ...extra,
    });
    const res = await fetch(`${url}/authorize?${query}`, { redirect: "manual" });
    const location = res.headers.get("location");
    return { res, verifier: pair.verifier, code: location ? new URL(location).searchParams.get("code") : null, location };
  }

  async function exchange(authz, key, extra = {}) {
    const headers = { ...FORM };
    if (key) headers.dpop = await proofAt(key, extra);
    return fetch(`${url}/token`, {
      method: "POST", headers,
      body: fields({ grant_type: "authorization_code", client_id: WEB, code: authz.code, code_verifier: authz.verifier, redirect_uri: redirect }),
    });
  }

  async function signIn(email = "dev@example.com", scope) {
    const key = await newKey();
    const authz = await authorize(email, {}, scope);
    const res = await exchange(authz, key);
    assert.equal(res.status, 200, await res.clone().text());
    return { key, tokens: await res.json() };
  }

  async function refresh(refreshToken, key, extra = {}) {
    const headers = { ...FORM };
    if (key) headers.dpop = await proofAt(key, extra);
    return fetch(`${url}/token`, { method: "POST", headers, body: fields({ grant_type: "refresh_token", client_id: WEB, refresh_token: refreshToken }) });
  }

  describe("discovery and authorize", () => {
    it("lists the endpoints a browser client reads", async () => {
      const doc = await (await fetch(`${url}/.well-known/openid-configuration`)).json();
      assert.equal(doc.revocation_endpoint, `${ISSUER}/revoke`);
      assert.equal(doc.end_session_endpoint, "http://127.0.0.1:8099/logout");
      assert.deepEqual(doc.dpop_signing_alg_values_supported, ["ES256"]);
      assert.ok(doc.grant_types_supported.includes("refresh_token"));
      assert.ok(doc.token_endpoint_auth_methods_supported.includes("none"));
      assert.ok(doc.scopes_supported.includes("offline_access"));
      assert.ok(doc.code_challenge_methods_supported.includes("S256"));
    });

    it("requires PKCE with S256, and only the redirect URIs of the client", async () => {
      const plain = fields({ client_id: WEB, redirect_uri: redirect, response_type: "code", login_hint: "dev@example.com" });
      assert.equal((await fetch(`${url}/authorize?${plain}`, { redirect: "manual" })).status, 400);
      const weak = fields({ client_id: WEB, redirect_uri: redirect, response_type: "code", code_challenge: "x", code_challenge_method: "plain" });
      assert.equal((await fetch(`${url}/authorize?${weak}`, { redirect: "manual" })).status, 400);
      const elsewhere = await authorize("dev@example.com", { redirect_uri: "http://evil.example/cb" });
      assert.equal(elsewhere.res.status, 400);
      const good = await authorize("dev@example.com");
      assert.equal(good.res.status, 302);
      assert.ok(good.code);
      assert.equal(new URL(good.location).searchParams.get("state"), "s1");
    });
  });

  describe("the code is redeemed with a DPoP proof", () => {
    it("gives tokens bound to the key: token_type DPoP, cnf.jkt, the confidential client's audience, the roles", async () => {
      const { key, tokens } = await signIn("admin@example.com");
      assert.equal(tokens.token_type, "DPoP");
      assert.equal(tokens.scope, "openid email profile offline_access");
      assert.equal(tokens.expires_in, 300);
      assert.equal(tokens.refresh_expires_in, 0, "an offline token has no expiry to show");
      const { keys } = await (await fetch(`${url}/jwks`)).json();
      for (const jwt of [tokens.access_token, tokens.id_token]) {
        const [head, body, signature] = jwt.split(".");
        assert.ok(createVerify("RSA-SHA256").update(`${head}.${body}`).verify(createPublicKey({ key: keys[0], format: "jwk" }), Buffer.from(signature, "base64url")));
      }
      const at = claimsOf(tokens.access_token);
      assert.equal(at.cnf.jkt, key.jkt);
      assert.equal(at.cnf.jkt, thumbprint(key.jwk));
      assert.deepEqual(at.aud, [WEB, "dev-chat"]);
      assert.equal(at.azp, WEB);
      assert.equal(at.email, "admin@example.com");
      assert.deepEqual(at.roles, ["admin"]);
      assert.equal(at.exp - at.iat, 300);
      const id = claimsOf(tokens.id_token);
      assert.equal(id.nonce, "n1");
      assert.equal(id.cnf, undefined, "an ID token is not bound");
    });

    it("an authorize with no offline_access scope gives a session refresh token", async () => {
      const { tokens } = await signIn("dev@example.com", "openid email");
      assert.equal(tokens.scope, "openid email");
      assert.equal(tokens.refresh_expires_in, 1800);
    });

    it("refuses a missing proof and every kind of bad one (invalid_dpop_proof), and consumes the code", async () => {
      const key = await newKey();
      const missing = await exchange(await authorize("dev@example.com"), null);
      assert.equal(missing.status, 400);
      assert.equal((await missing.json()).error, "invalid_dpop_proof");
      const bad = {
        "typ": { header: { typ: "JWT" } },
        "alg": { header: { alg: "HS256" } },
        "no jwk": { header: { jwk: undefined } },
        "a jwk with the private key": { header: { jwk: { ...key.jwk, d: "AAAA" } } },
        "another htm": { claims: { htm: "GET" } },
        "another htu": { url: `${ISSUER}/revoke` },
        "no jti": { claims: { jti: undefined } },
        "too old": { claims: { iat: time - 26 } },
        "in the future": { claims: { iat: time + 16 } },
        "no iat": { claims: { iat: undefined } },
      };
      for (const [name, extra] of Object.entries(bad)) {
        const res = await exchange(await authorize("dev@example.com"), key, extra);
        assert.equal(res.status, 400, name);
        assert.equal((await res.json()).error, "invalid_dpop_proof", name);
      }
      const authz = await authorize("dev@example.com");
      const forged = (await proofAt(key)).replace(/\.[^.]+$/, `.${Buffer.alloc(64).toString("base64url")}`);
      const res = await fetch(`${url}/token`, {
        method: "POST", headers: { ...FORM, dpop: forged },
        body: fields({ grant_type: "authorization_code", client_id: WEB, code: authz.code, code_verifier: authz.verifier }),
      });
      assert.equal((await res.json()).error, "invalid_dpop_proof", "a signature that does not verify");
      const again = await exchange(authz, key);
      assert.equal((await again.json()).error, "invalid_grant", "the code was used up by the attempt");
    });

    it("takes a proof within the window (10 s and 15 s of skew back, 15 s ahead) and each jti once", async () => {
      const key = await newKey();
      for (const iat of [time - 25, time + 15]) {
        const res = await exchange(await authorize("dev@example.com"), key, { claims: { iat } });
        assert.equal(res.status, 200, `iat ${iat - time}`);
      }
      const proof = await proofAt(key);
      const send = async () => {
        const authz = await authorize("dev@example.com");
        return fetch(`${url}/token`, {
          method: "POST", headers: { ...FORM, dpop: proof },
          body: fields({ grant_type: "authorization_code", client_id: WEB, code: authz.code, code_verifier: authz.verifier }),
        });
      };
      assert.equal((await send()).status, 200);
      const replay = await send();
      assert.equal(replay.status, 400);
      assert.match((await replay.json()).error_description, /already used/);
      time += 120; // the memory of a jti is not kept forever
      assert.equal((await send()).status, 400, "an old proof is refused for its age, not only its jti");
    });

    it("takes a wrong PKCE verifier, and a code of another redirect, as invalid_grant", async () => {
      const key = await newKey();
      const authz = await authorize("dev@example.com");
      const res = await exchange({ ...authz, verifier: "wrong" }, key);
      assert.equal((await res.json()).error, "invalid_grant");
      const other = await authorize("dev@example.com");
      const res2 = await fetch(`${url}/token`, {
        method: "POST", headers: { ...FORM, dpop: await proofAt(key) },
        body: fields({ grant_type: "authorization_code", client_id: WEB, code: other.code, code_verifier: other.verifier, redirect_uri: "http://edge:8080/other" }),
      });
      assert.equal((await res2.json()).error, "invalid_grant");
    });

    it("binds the code to a key named by dpop_jkt at authorize (RFC 9449 section 10)", async () => {
      const key = await newKey();
      const intruder = await newKey();
      const authz = await authorize("dev@example.com", { dpop_jkt: key.jkt });
      const stolen = await exchange(authz, intruder);
      assert.equal((await stolen.json()).error, "invalid_grant");
      const authz2 = await authorize("dev@example.com", { dpop_jkt: key.jkt });
      assert.equal((await exchange(authz2, key)).status, 200);
    });

    it("does not let the confidential client redeem a code of the public one, nor the public one take client_credentials", async () => {
      const authz = await authorize("dev@example.com");
      const res = await post("/token", { grant_type: "authorization_code", code: authz.code, code_verifier: authz.verifier, ...creds });
      assert.equal(res.status, 400);
      assert.equal((await res.json()).error, "invalid_grant");
      const cc = await fetch(`${url}/token`, { method: "POST", headers: FORM, body: fields({ grant_type: "client_credentials", client_id: WEB }) });
      assert.equal(cc.status, 400);
      assert.equal((await cc.json()).error, "unauthorized_client");
    });
  });

  describe("the refresh token is good once", () => {
    it("rotates: a new access token and a new refresh token, bound to the same key", async () => {
      const { key, tokens } = await signIn();
      const res = await refresh(tokens.refresh_token, key);
      assert.equal(res.status, 200);
      const next = await res.json();
      assert.notEqual(next.refresh_token, tokens.refresh_token);
      assert.notEqual(next.access_token, tokens.access_token);
      assert.equal(next.token_type, "DPoP");
      assert.equal(claimsOf(next.access_token).cnf.jkt, key.jkt);
      assert.equal(claimsOf(next.access_token).email, "dev@example.com");
      assert.ok(claimsOf(next.id_token).nonce === undefined);
      assert.equal((await refresh(next.refresh_token, key)).status, 200, "the new token refreshes in its turn");
    });

    it("a refresh token used twice ends the whole session: the old one and the new one", async () => {
      const { key, tokens } = await signIn();
      const next = await (await refresh(tokens.refresh_token, key)).json();
      const reuse = await refresh(tokens.refresh_token, key);
      assert.equal(reuse.status, 400);
      const body = await reuse.json();
      assert.equal(body.error, "invalid_grant");
      assert.match(body.error_description, /revoked/);
      const dead = await refresh(next.refresh_token, key);
      assert.equal(dead.status, 400);
      assert.equal((await dead.json()).error, "invalid_grant");
    });

    it("a refresh with no proof, or another key's, is invalid_grant and uses nothing up", async () => {
      const { key, tokens } = await signIn();
      const none = await refresh(tokens.refresh_token, null);
      assert.equal((await none.json()).error, "invalid_grant");
      const other = await refresh(tokens.refresh_token, await newKey());
      assert.equal((await other.json()).error, "invalid_grant");
      assert.equal((await refresh(tokens.refresh_token, key)).status, 200, "the token is still good for its key");
    });

    it("a refresh with a malformed proof is invalid_dpop_proof; with an unknown token invalid_grant", async () => {
      const { key, tokens } = await signIn();
      const bad = await refresh(tokens.refresh_token, key, { claims: { htm: "GET" } });
      assert.equal((await bad.json()).error, "invalid_dpop_proof");
      assert.equal((await refresh("nope", key)).status, 400);
    });

    it("a refresh token expires: a session's after 30 minutes, an offline one's after 30 days, counted from its last use", async () => {
      const session = await signIn("dev@example.com", "openid email");
      const offline = await signIn("dev@example.com");
      time += 1801;
      assert.equal((await (await refresh(session.tokens.refresh_token, session.key)).json()).error, "invalid_grant");
      const used = await refresh(offline.tokens.refresh_token, offline.key);
      assert.equal(used.status, 200, "an offline token outlives the session's");
      time += 29 * 24 * 3600; // 29 days after that use: the new token is good for 30 from it
      const again = await refresh((await used.json()).refresh_token, offline.key);
      assert.equal(again.status, 200);
      time += 31 * 24 * 3600;
      assert.equal((await (await refresh((await again.json()).refresh_token, offline.key)).json()).error, "invalid_grant");
    });
  });

  describe("revocation (RFC 7009) and end_session", () => {
    const revoke = async (token, key, extra = {}, fieldsMore = {}) => {
      const headers = { ...FORM };
      if (key) headers.dpop = await makeProof(key, { method: "POST", url: `${ISSUER}/revoke`, ...extra, claims: { iat: time, ...(extra.claims ?? {}) } });
      return fetch(`${url}/revoke`, { method: "POST", headers, body: fields({ client_id: WEB, token, token_type_hint: "refresh_token", ...fieldsMore }) });
    };

    it("revokes the session of a refresh token, given a proof of its key", async () => {
      const { key, tokens } = await signIn();
      assert.equal((await revoke(tokens.refresh_token, null)).status, 400, "a bound token needs a proof");
      assert.equal((await revoke(tokens.refresh_token, await newKey())).status, 400, "another key's proof");
      const next = await (await refresh(tokens.refresh_token, key)).json();
      assert.ok(next.refresh_token, "neither revoked anything: the token still refreshes");
      assert.equal((await revoke(next.refresh_token, key)).status, 200);
      const dead = await refresh(next.refresh_token, key);
      assert.equal(dead.status, 400);
      assert.equal((await dead.json()).error, "invalid_grant");
    });

    it("answers 200 for a token nobody knows and for an access token, and 400 with no token", async () => {
      const key = await newKey();
      assert.equal((await revoke("nobody", key)).status, 200);
      const { tokens } = await signIn();
      assert.equal((await revoke(tokens.access_token, key, {}, { token_type_hint: "access_token" })).status, 200);
      assert.equal((await revoke("", key)).status, 400);
      assert.equal((await fetch(`${url}/revoke`)).status, 405);
    });

    it("end_session redirects to the post-logout page with the state, or says it is done", async () => {
      const res = await fetch(`${url}/logout?${fields({ post_logout_redirect_uri: `${origin}/`, state: "bye" })}`, { redirect: "manual" });
      assert.equal(res.status, 302);
      assert.equal(res.headers.get("location"), `${origin}/?state=bye`);
      assert.match(res.headers.get("set-cookie"), /mock_oidc_user=;/);
      assert.equal((await fetch(`${url}/logout`)).status, 200);
      assert.equal((await fetch(`${url}/logout?post_logout_redirect_uri=javascript:alert(1)`, { redirect: "manual" })).status, 400);
    });
  });

  describe("userinfo", () => {
    const info = async (token, scheme, key, extra = {}) => {
      const headers = { authorization: `${scheme} ${token}` };
      if (key) headers.dpop = await makeProof(key, { method: "GET", url: `${ISSUER}/userinfo`, accessToken: token, ...extra, claims: { iat: time, ...(extra.claims ?? {}) } });
      return fetch(`${url}/userinfo`, { headers });
    };

    it("answers a bound token only to the DPoP scheme with a proof of its key", async () => {
      const { key, tokens } = await signIn("admin@example.com");
      assert.equal((await info(tokens.access_token, "Bearer", null)).status, 401);
      assert.equal((await info(tokens.access_token, "DPoP", null)).status, 401);
      assert.equal((await info(tokens.access_token, "DPoP", await newKey())).status, 401);
      assert.equal((await info(tokens.access_token, "DPoP", key, { claims: { ath: "x" } })).status, 401);
      const ok = await info(tokens.access_token, "DPoP", key);
      assert.equal(ok.status, 200);
      assert.equal((await ok.json()).email, "admin@example.com");
      const plain = await (await fetch(`${url}/token`, { method: "POST", headers: { ...FORM, authorization: `Basic ${Buffer.from("dev-chat:dev-client-secret").toString("base64")}` }, body: fields({ grant_type: "client_credentials" }) })).json();
      assert.equal((await info(plain.access_token, "Bearer", null)).status, 200, "a token with no cnf is as before");
      assert.equal((await info(plain.access_token, "DPoP", key)).status, 401, "and is not sent with DPoP");
    });
  });

  describe("CORS for the browser's origin", () => {
    it("answers the listed origin on the endpoints a page calls, and no other origin", async () => {
      for (const path of ["/.well-known/openid-configuration", "/jwks", "/userinfo"]) {
        const yes = await fetch(url + path, { headers: { origin } });
        assert.equal(yes.headers.get("access-control-allow-origin"), origin, path);
        assert.match(yes.headers.get("vary"), /Origin/);
        const no = await fetch(url + path, { headers: { origin: "http://evil.example" } });
        assert.equal(no.headers.get("access-control-allow-origin"), null, path);
      }
      const post = await fetch(`${url}/token`, { method: "POST", headers: { ...FORM, origin }, body: "" });
      assert.equal(post.headers.get("access-control-allow-origin"), origin, "an error carries the header too: the page can read it");
      assert.equal((await fetch(`${url}/healthz`, { headers: { origin } })).headers.get("access-control-allow-origin"), null);
    });

    it("answers the preflight of a DPoP request for the listed origin only", async () => {
      const preflight = (from) => fetch(`${url}/token`, {
        method: "OPTIONS",
        headers: { origin: from, "access-control-request-method": "POST", "access-control-request-headers": "content-type,dpop" },
      });
      const yes = await preflight(origin);
      assert.equal(yes.status, 204);
      assert.equal(yes.headers.get("access-control-allow-origin"), origin);
      assert.match(yes.headers.get("access-control-allow-methods"), /POST/);
      assert.equal(yes.headers.get("access-control-allow-headers"), "content-type,dpop");
      const no = await preflight("http://evil.example");
      assert.equal(no.headers.get("access-control-allow-origin"), null);
    });
  });
});

describe("without the public client", () => {
  it("is the issuer of before: no revocation, no refresh grant, and dev-web is an unknown client", async () => {
    const plain = createMockServer({ users, issuer: ISSUER, webClientId: "" });
    await new Promise((resolve) => plain.listen(0, "127.0.0.1", resolve));
    const at = `http://127.0.0.1:${plain.address().port}`;
    try {
      const doc = await (await fetch(`${at}/.well-known/openid-configuration`)).json();
      assert.equal(doc.revocation_endpoint, undefined);
      assert.deepEqual(doc.grant_types_supported, ["authorization_code", "client_credentials"]);
      assert.deepEqual(doc.scopes_supported, ["openid", "email", "profile"]);
      const res = await fetch(`${at}/authorize?client_id=dev-web&redirect_uri=http://x/cb&response_type=code&code_challenge=x&code_challenge_method=S256`, { redirect: "manual" });
      assert.equal(res.status, 400);
      assert.equal((await fetch(`${at}/revoke`, { method: "POST" })).status, 404);
    } finally {
      plain.close();
      plain.closeAllConnections();
    }
  });
});
