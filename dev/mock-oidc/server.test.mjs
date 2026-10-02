// node --test dev/mock-oidc/server.test.mjs
// Holds the mock issuer to what the orchestrator (docs/api/config.md, "Authentication") and oauth2-proxy read of an issuer:
// discovery, the key set, the code flow with PKCE, client_credentials with a user, and the claims of the token.
import assert from "node:assert/strict";
import { createHash, createPublicKey, createVerify } from "node:crypto";
import { readFileSync } from "node:fs";
import { after, before, describe, it } from "node:test";
import { createMockServer, subjectOf } from "./server.mjs";

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
