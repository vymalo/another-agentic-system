// The browser web's sign-in and DPoP, played by a script (dev/browser-auth-e2e.sh; ADR 0054). It does what the web does with `oauth4webapi`, with
// node's own WebCrypto and fetch and no dependencies: ask the orchestrator which issuer to sign in at, authorize with PKCE at the mock issuer's
// public client, redeem the code with a DPoP proof, call the API through the edge with `Authorization: DPoP`, refresh (a refresh token is good
// once), revoke. Every request through the edge is sent to EDGE_URL, and every proof says PUBLIC_ORIGIN (the origin the orchestrator's
// `auth.dpop.publicOrigins` names): the address a request is sent to and the one a proof names are two things, as behind a proxy.
//
// Run it where `edge` and `mock-oidc` resolve (the scenario runs it in the `mock-oidc` container, which has node and no other tool):
//
//   docker compose --profile app exec -T mock-oidc node /browser-auth/flow.mjs
//
// or from the host, naming where the issuer is reached (`http://mock-oidc:8080` is the `iss` and the address inside compose only):
//
//   OIDC_REACH=http://127.0.0.1:8099 EDGE_URL=http://127.0.0.1:8080 PUBLIC_ORIGIN=http://edge:8080 node dev/browser-auth/flow.mjs
//
// Environment (all optional):
//   EDGE_URL        http://edge:8080        where the API is reached (the compose `edge`)
//   PUBLIC_ORIGIN   the origin of EDGE_URL  the origin of every proof's `htu`: one of `auth.dpop.publicOrigins` (dev/orchestrator.browser-auth.yaml)
//   OIDC_ISSUER     (from GET /api/public/auth)  the issuer, when ISSUER_ONLY is set
//   OIDC_REACH      (none)                  the address to reach the issuer's endpoints at, in place of the origins discovery names
//   USER_EMAIL      dev@example.com         the person who signs in; OTHER_EMAIL (someone-else@example.com) is another person
//   AGENT_ID        mock-coder              the agent of the AG-UI run (a WireMock agent: it answers at once)
//   ISSUER_ONLY     (unset)                 1: play only what the issuer does (what needs no orchestrator): the sign-in, the refresh and the revocation
//   TIMEOUT         60                      seconds to wait for a thread to end
//
// It prints one ok or FAIL line per check and exits 1 if any failed. The helpers below are exported for the issuer's tests
// (dev/mock-oidc/server.test.mjs).
import { createHash, randomBytes, webcrypto } from "node:crypto";
import { pathToFileURL } from "node:url";

const { subtle } = webcrypto;
const b64url = (data) => Buffer.from(data).toString("base64url");
const text = (value) => Buffer.from(JSON.stringify(value));

/** A DPoP key: ECDSA P-256, with its public JWK (RFC 7517) and its RFC 7638 thumbprint. */
export async function newKey() {
  const pair = await subtle.generateKey({ name: "ECDSA", namedCurve: "P-256" }, true, ["sign", "verify"]);
  const { kty, crv, x, y } = await subtle.exportKey("jwk", pair.publicKey);
  const jwk = { kty, crv, x, y };
  const jkt = b64url(createHash("sha256").update(JSON.stringify({ crv, kty, x, y })).digest());
  return { privateKey: pair.privateKey, jwk, jkt };
}

/**
 * A DPoP proof (RFC 9449 section 4.2) for `method` and `url` (no query: it is cut), with `ath` when an access token is named. `claims` and
 * `header` replace members, to make a bad proof; a member set to `undefined` is left out.
 */
export async function makeProof(key, { method, url, accessToken, claims = {}, header = {} }) {
  const target = new URL(url);
  const head = { typ: "dpop+jwt", alg: "ES256", jwk: key.jwk, ...header };
  const body = {
    jti: b64url(randomBytes(16)),
    htm: method,
    htu: `${target.origin}${target.pathname}`,
    iat: Math.floor(Date.now() / 1000),
    ...(accessToken === undefined ? {} : { ath: b64url(createHash("sha256").update(accessToken).digest()) }),
    ...claims,
  };
  const data = `${b64url(text(head))}.${b64url(text(body))}`;
  const signature = await subtle.sign({ name: "ECDSA", hash: "SHA-256" }, key.privateKey, Buffer.from(data));
  return `${data}.${b64url(signature)}`;
}

/** A PKCE pair (RFC 7636, S256). */
export function pkce() {
  const verifier = b64url(randomBytes(32));
  return { verifier, challenge: b64url(createHash("sha256").update(verifier).digest()) };
}

/** The claims of a JWT, not verified (the issuer's own tests verify the signature). */
export const claimsOf = (jwt) => JSON.parse(Buffer.from(String(jwt).split(".")[1], "base64url").toString("utf8"));

/** One HTTP exchange: the status, the headers, the text and, when it is JSON, the value. */
export async function call(method, url, { headers = {}, body, redirect = "manual", timeoutMs = 60_000 } = {}) {
  const response = await fetch(url, { method, headers, body, redirect, signal: AbortSignal.timeout(timeoutMs) });
  const raw = await response.text();
  let json;
  try {
    json = JSON.parse(raw);
  } catch {
    json = undefined;
  }
  return { status: response.status, headers: response.headers, text: raw, json };
}

const form = (fields) => new URLSearchParams(fields).toString();
const FORM = { "content-type": "application/x-www-form-urlencoded" };

/** The flow against a deployment: `edge` and `origin` for the API, `issuer` for sign-in, `reach` where the issuer's endpoints are reached. */
export function client({ edge, origin, issuer, reach, clientId, redirectUri, scope }) {
  let discovery;
  const toReach = (value) => {
    const url = new URL(value);
    const base = new URL(reach ?? issuer);
    url.protocol = base.protocol;
    url.host = base.host;
    return url.toString();
  };
  const endpoints = async () => {
    if (!discovery) {
      const found = await call("GET", toReach(`${issuer}/.well-known/openid-configuration`));
      if (found.status !== 200 || !found.json) throw new Error(`discovery answered ${found.status}: ${found.text.slice(0, 200)}`);
      discovery = found.json;
    }
    return discovery;
  };

  /** Authorize as `email` with PKCE and return the code (and the pieces to redeem it); a refusal is `{ status, text }`. */
  async function authorize(email, { challenge, verifier, withPkce = true, state = b64url(randomBytes(8)), nonce = b64url(randomBytes(8)), extra = {} } = {}) {
    const doc = await endpoints();
    const pair = challenge ? { challenge, verifier } : pkce();
    const query = {
      client_id: clientId,
      redirect_uri: redirectUri,
      response_type: "code",
      scope,
      state,
      nonce,
      login_hint: email,
      ...(withPkce ? { code_challenge: pair.challenge, code_challenge_method: "S256" } : {}),
      ...extra,
    };
    const answer = await call("GET", `${toReach(doc.authorization_endpoint)}?${form(query)}`);
    if (answer.status !== 302) return { status: answer.status, text: answer.text };
    const back = new URL(answer.headers.get("location"));
    return { status: 302, code: back.searchParams.get("code"), state: back.searchParams.get("state"), sentState: state, iss: back.searchParams.get("iss"), nonce, verifier: pair.verifier, back };
  }

  /** The token endpoint, with a proof of `key` (or none, or the one given). */
  async function token(fields, { key, proof, htu, claims, header } = {}) {
    const doc = await endpoints();
    const headers = { ...FORM };
    const proofJwt = proof ?? (key ? await makeProof(key, { method: "POST", url: htu ?? doc.token_endpoint, claims, header }) : undefined);
    if (proofJwt) headers.dpop = proofJwt;
    return call("POST", toReach(doc.token_endpoint), { headers, body: form({ client_id: clientId, ...fields }) });
  }

  /** Sign `email` in and redeem the code: a session `{ key, tokens }` or the refusal that came first. */
  async function login(email, { key, tokenOptions } = {}) {
    const own = key ?? (await newKey());
    const asked = await authorize(email);
    if (asked.status !== 302) return { refused: asked };
    const redeemed = await token(
      { grant_type: "authorization_code", code: asked.code, code_verifier: asked.verifier, redirect_uri: redirectUri },
      { key: own, ...tokenOptions },
    );
    return { key: own, asked, redeemed, tokens: redeemed.json };
  }

  const refresh = (refreshToken, options) => token({ grant_type: "refresh_token", refresh_token: refreshToken }, options);

  async function revoke(refreshToken, options = {}) {
    const doc = await endpoints();
    const headers = { ...FORM };
    const proof = options.proof ?? (options.key ? await makeProof(options.key, { method: "POST", url: doc.revocation_endpoint }) : undefined);
    if (proof) headers.dpop = proof;
    return call("POST", toReach(doc.revocation_endpoint), { headers, body: form({ client_id: clientId, token: refreshToken, token_type_hint: "refresh_token" }) });
  }

  /** A request to the orchestrator through the edge with `Authorization: DPoP` and a fresh proof (overridable), for `session`. */
  async function api(method, path, session, { body, headers = {}, proof, claims, header, scheme = "DPoP", accessToken, key, htu, redirect } = {}) {
    const token = accessToken ?? session.tokens.access_token;
    const sign = key ?? session.key;
    const jwt = proof === null ? undefined : (proof ?? (await makeProof(sign, { method, url: htu ?? `${origin}${path.split("?")[0]}`, accessToken: token, claims, header })));
    return call(method, `${edge}${path}`, {
      headers: { authorization: `${scheme} ${token}`, ...(jwt ? { dpop: jwt } : {}), ...headers },
      body,
      redirect,
    });
  }

  return { authorize, token, login, refresh, revoke, api, endpoints, toReach };
}

// ---------------------------------------------------------------------------------------------------------------------------------

const uuid = () => webcrypto.randomUUID();

async function main() {
  const edge = (process.env.EDGE_URL ?? "http://edge:8080").replace(/\/+$/, "");
  const origin = (process.env.PUBLIC_ORIGIN ?? new URL(edge).origin).replace(/\/+$/, "");
  const email = process.env.USER_EMAIL ?? "dev@example.com";
  const other = process.env.OTHER_EMAIL ?? "someone-else@example.com";
  const agent = process.env.AGENT_ID ?? "mock-coder";
  const issuerOnly = process.env.ISSUER_ONLY === "1";
  const timeout = Number(process.env.TIMEOUT ?? 60);
  let failed = 0;
  const ok = (message) => console.log(`ok   ${message}`);
  const bad = (message) => {
    console.log(`FAIL ${message}`);
    failed += 1;
  };
  const expect = (message, condition, detail = "") => (condition ? ok(message) : bad(`${message}${detail === "" ? "" : `: ${detail}`}`));
  const brief = (answer) => `HTTP ${answer.status} ${answer.text.slice(0, 200)}`;
  const challenge = (answer) => String(answer.headers.get("www-authenticate") ?? "");

  // --- 1. the issuer the web signs in at: the orchestrator says it, with no sign-in -------------------------------------------------
  let config = { issuer: process.env.OIDC_ISSUER ?? "http://mock-oidc:8080", clientId: "dev-web", scope: "openid email profile offline_access" };
  if (!issuerOnly) {
    const told = await call("GET", `${edge}/api/public/auth`);
    expect("GET /api/public/auth answers 200 with no sign-in", told.status === 200 && told.json, brief(told));
    config = told.json ?? config;
    expect("... {issuer, clientId, scope}: the issuer is an http(s) URL, the client is the public client, the scope asks for offline_access",
      /^https?:\/\//.test(config.issuer ?? "") && typeof config.clientId === "string" && config.clientId !== "" && String(config.scope).split(" ").includes("offline_access") && String(config.scope).split(" ").includes("openid"),
      JSON.stringify(config));
    expect("... and nothing else (no secret, no endpoint of the orchestrator)", Object.keys(config).sort().join(",") === "clientId,issuer,scope", Object.keys(config).join(","));
    const hostile = await call("GET", `${edge}/api/public/auth`, { headers: { authorization: "Bearer not-a-token", "x-auth-request-email": "admin@example.com" } });
    expect("... the same with a bogus Authorization and an identity header (the edge strips both: it does not matter who asks)", hostile.status === 200 && hostile.json?.issuer === config.issuer, brief(hostile));
    const post = await call("POST", `${edge}/api/public/auth`, { headers: { "content-type": "application/json" }, body: "{}" });
    expect("... POST is not part of the public route (it falls to the sign-in: 401)", post.status === 401 || post.status === 405 || post.status === 404, brief(post));
  }

  const redirectUri = process.env.REDIRECT_URI ?? `${origin}/auth/callback`;
  const web = client({ edge, origin, issuer: config.issuer, reach: process.env.OIDC_REACH, clientId: config.clientId, redirectUri, scope: config.scope });
  const doc = await web.endpoints();
  expect("discovery names the issuer and its public-client endpoints (revocation, end_session, DPoP with ES256)",
    doc.issuer === config.issuer && doc.revocation_endpoint && doc.end_session_endpoint && (doc.dpop_signing_alg_values_supported ?? []).includes("ES256") &&
      (doc.grant_types_supported ?? []).includes("refresh_token") && (doc.code_challenge_methods_supported ?? []).includes("S256"),
    JSON.stringify(doc).slice(0, 300));

  // --- 2. authorization code with PKCE; the code is redeemed with a proof; the tokens are bound to the key ---------------------------
  const nopkce = await web.authorize(email, { withPkce: false });
  expect("authorize without PKCE is refused (a public client)", nopkce.status === 400, `HTTP ${nopkce.status} ${nopkce.text ?? ""}`.slice(0, 200));
  const asked = await web.authorize(email);
  expect("authorize as the person named by login_hint answers a code and the state, with the issuer", asked.status === 302 && asked.code && asked.state === asked.sentState && asked.iss === config.issuer, JSON.stringify({ ...asked, back: undefined, verifier: undefined }));
  const naked = await web.token({ grant_type: "authorization_code", code: asked.code, code_verifier: asked.verifier, redirect_uri: redirectUri });
  expect("the code redeemed with no DPoP proof is refused (400 invalid_dpop_proof): the client's tokens are bound", naked.status === 400 && naked.json?.error === "invalid_dpop_proof", brief(naked));
  const second = await web.authorize(email);
  const wrongVerifier = await web.token({ grant_type: "authorization_code", code: second.code, code_verifier: "not-the-verifier", redirect_uri: redirectUri }, { key: await newKey() });
  expect("a wrong PKCE verifier is invalid_grant", wrongVerifier.status === 400 && wrongVerifier.json?.error === "invalid_grant", brief(wrongVerifier));
  const third = await web.authorize(email);
  const badHtu = await web.token({ grant_type: "authorization_code", code: third.code, code_verifier: third.verifier, redirect_uri: redirectUri }, { key: await newKey(), htu: `${config.issuer}/somewhere-else` });
  expect("a proof for another htu is refused (invalid_dpop_proof)", badHtu.status === 400 && badHtu.json?.error === "invalid_dpop_proof", brief(badHtu));

  const session = await web.login(email);
  const t = session.tokens ?? {};
  expect("the code redeemed with a proof gives tokens: token_type DPoP, an access token, an ID token and a refresh token", session.redeemed?.status === 200 && t.token_type === "DPoP" && t.access_token && t.id_token && t.refresh_token, brief(session.redeemed ?? { status: 0, text: "" }));
  if (!t.access_token) {
    console.log("browser-auth flow FAILED (no tokens: nothing else can run)");
    process.exit(1);
  }
  const at = claimsOf(t.access_token);
  expect("the access token is bound to the key (cnf.jkt is the RFC 7638 thumbprint of the proof's key)", at.cnf?.jkt === session.key.jkt, JSON.stringify(at.cnf));
  expect("... it is the person's: email, verified, the roles claim, and the orchestrator's audience", at.email === email && at.email_verified === true && Array.isArray(at.roles) && (Array.isArray(at.aud) ? at.aud : [at.aud]).includes("dev-chat"), JSON.stringify({ email: at.email, roles: at.roles, aud: at.aud }));
  expect("... it lasts minutes, not an hour (the web's client has its own lifespan)", at.exp - at.iat > 0 && at.exp - at.iat <= 600, `${at.exp - at.iat} s`);
  expect("the ID token carries the nonce of the authorize request", claimsOf(t.id_token).nonce === session.asked.nonce);
  expect("the scope says offline_access (an offline refresh token)", String(t.scope).split(" ").includes("offline_access"), t.scope);

  // --- 3. the API through the edge, DPoP ----------------------------------------------------------------------------------------------
  if (!issuerOnly) {
    const me = await web.api("GET", "/api/me", session);
    expect("GET /api/me through the edge with Authorization: DPoP and a proof is 200 and the person's", me.status === 200 && me.json?.user === email, brief(me));
    expect("... with roles and permissions (the orchestrator resolved the token's roles)", Array.isArray(me.json?.roles) && me.json.roles.length > 0 && Array.isArray(me.json?.permissions) && me.json.permissions.length > 0, JSON.stringify(me.json));
    const smuggled = await web.api("GET", "/api/me", session, { headers: { "x-auth-request-email": "admin@example.com" } });
    expect("an identity header sent beside a DPoP request changes nothing (the edge removes it, the orchestrator reads none)", smuggled.status === 200 && smuggled.json?.user === email, brief(smuggled));
    const query = await web.api("GET", "/api/agents?x=1", session);
    expect("a query string is not part of htu: the proof of the path alone is good", query.status === 200, brief(query));

    // the token as a Bearer: a bound token without its proof is no credential (RFC 9449 section 7.1)
    const asBearer = await call("GET", `${edge}/api/me`, { headers: { authorization: `Bearer ${t.access_token}` } });
    expect("the same token sent as Bearer is 401", asBearer.status === 401, brief(asBearer));
    const asBearerProof = await web.api("GET", "/api/me", session, { scheme: "Bearer" });
    expect("... with its proof beside it too", asBearerProof.status === 401, brief(asBearerProof));
    const noProof = await web.api("GET", "/api/me", session, { proof: null });
    expect("Authorization: DPoP with no DPoP header is 401", noProof.status === 401, brief(noProof));
    expect("... and says DPoP in WWW-Authenticate", /^DPoP /i.test(challenge(noProof)), challenge(noProof));

    // a proof is good once, for one request
    const proof = await makeProof(session.key, { method: "GET", url: `${origin}/api/me`, accessToken: t.access_token });
    const first = await web.api("GET", "/api/me", session, { proof });
    const replay = await web.api("GET", "/api/me", session, { proof });
    expect("a proof is good once: the first use is 200 and the replay is 401", first.status === 200 && replay.status === 401, `${first.status} then ${replay.status}`);
    expect("... the refusal says invalid_dpop_proof", /invalid_dpop_proof/.test(challenge(replay)), challenge(replay));
    const wrongHtu = await web.api("GET", "/api/me", session, { htu: `${origin}/api/agents` });
    expect("a proof for another path (htu) is 401", wrongHtu.status === 401 && /invalid_dpop_proof/.test(challenge(wrongHtu)), `${brief(wrongHtu)} ${challenge(wrongHtu)}`);
    const wrongOrigin = await web.api("GET", "/api/me", session, { htu: "https://elsewhere.example.org/api/me" });
    expect("a proof for another origin is 401", wrongOrigin.status === 401, brief(wrongOrigin));
    const wrongMethod = await web.api("GET", "/api/me", session, { claims: { htm: "POST" } });
    expect("a proof for another method (htm) is 401", wrongMethod.status === 401, brief(wrongMethod));
    const stale = await web.api("GET", "/api/me", session, { claims: { iat: Math.floor(Date.now() / 1000) - 300 } });
    expect("a proof from five minutes ago (iat) is 401", stale.status === 401, brief(stale));
    const early = await web.api("GET", "/api/me", session, { claims: { iat: Math.floor(Date.now() / 1000) + 120 } });
    expect("a proof from two minutes ahead (iat) is 401", early.status === 401, brief(early));
    const wrongAth = await web.api("GET", "/api/me", session, { claims: { ath: b64url(createHash("sha256").update("another token").digest()) } });
    expect("a proof whose ath hashes another token is 401", wrongAth.status === 401, brief(wrongAth));
    const thief = await newKey();
    const stolen = await web.api("GET", "/api/me", session, { key: thief });
    expect("the token with a proof of ANOTHER key (a stolen token) is 401", stolen.status === 401, brief(stolen));
    const noTyp = await web.api("GET", "/api/me", session, { header: { typ: "JWT" } });
    expect("a proof whose typ is not dpop+jwt is 401", noTyp.status === 401, brief(noTyp));
    const hs = await web.api("GET", "/api/me", session, { header: { alg: "HS256" } });
    expect("a proof whose alg is not an asymmetric one the orchestrator allows is 401", hs.status === 401, brief(hs));
    const unbound = await call("POST", web.toReach(`${config.issuer}/token`), {
      headers: { ...FORM, authorization: `Basic ${Buffer.from("dev-chat:dev-client-secret").toString("base64")}` },
      body: form({ grant_type: "client_credentials", user: email }),
    });
    const plain = unbound.json?.access_token;
    if (plain) {
      const dpopWithPlain = await web.api("GET", "/api/me", session, { accessToken: plain });
      expect("a token with no cnf sent as DPoP is 401 (a DPoP request needs a bound token)", dpopWithPlain.status === 401, brief(dpopWithPlain));
      const plainBearer = await call("GET", `${edge}/api/me`, { headers: { authorization: `Bearer ${plain}` } });
      expect("... and as Bearer it is the cookie-less script of before: 200 (nothing changed for a token with no cnf)", plainBearer.status === 200 && plainBearer.json?.user === email, brief(plainBearer));
    } else {
      bad(`no client_credentials token to compare with: ${brief(unbound)}`);
    }

    // --- 4. AG-UI through the edge, DPoP: a run and a connect, and nobody else's thread ---------------------------------------------
    const stranger = await web.login(other);
    const thread = uuid();
    const input = JSON.stringify({ threadId: thread, runId: uuid(), state: {}, tools: [], context: [], messages: [{ id: uuid(), role: "user", content: "hello over DPoP" }], forwardedProps: {} });
    const run = await web.api("POST", `/agui/agents/${agent}`, session, { body: input, headers: { "content-type": "application/json", accept: "text/event-stream" } });
    expect(`POST /agui/agents/${agent} with DPoP is 200, an event stream that starts a run`, run.status === 200 && /RUN_STARTED/.test(run.text), brief(run));
    let state = "";
    for (let n = 0; n < timeout * 2; n += 1) {
      const got = await web.api("GET", `/api/threads/${thread}`, session);
      state = got.json?.state ?? "";
      if (["done", "blocked", "failed", "cancelled"].includes(state)) break;
      await new Promise((resolve) => setTimeout(resolve, 500));
    }
    expect("the thread is the person's and ends done", state === "done", state || "no state");
    const connect = await web.api("GET", `/agui/threads/${thread}/connect?mode=run`, session, { headers: { accept: "text/event-stream" } });
    expect("GET /agui/threads/{id}/connect with DPoP is 200, an event stream (the proof's htu has no query)", connect.status === 200 && /^text\/event-stream/.test(connect.headers.get("content-type") ?? "") && /RUN_STARTED/.test(connect.text), brief(connect));
    const intruder = await web.api("GET", `/api/threads/${thread}`, stranger);
    expect("another person's DPoP token does not read that thread (404)", intruder.status === 404, brief(intruder));
    const intruderStream = await web.api("GET", `/agui/threads/${thread}/connect?mode=run`, stranger, { headers: { accept: "text/event-stream" } });
    expect("... nor follow it (404)", intruderStream.status === 404, brief(intruderStream));
    const meOther = await web.api("GET", "/api/me", stranger);
    expect("... and /api/me of that person says so", meOther.status === 200 && meOther.json?.user === other, brief(meOther));
  }

  // --- 5. the refresh token is used once ------------------------------------------------------------------------------------------
  const ring = await web.login(email);
  const rt1 = ring.tokens.refresh_token;
  const refreshed = await web.refresh(rt1, { key: ring.key });
  const r = refreshed.json ?? {};
  expect("refresh with the key gives new tokens (a new access token, a new refresh token, still DPoP)", refreshed.status === 200 && r.access_token && r.access_token !== ring.tokens.access_token && r.refresh_token && r.refresh_token !== rt1 && r.token_type === "DPoP", brief(refreshed));
  expect("... the new access token is bound to the same key", claimsOf(r.access_token ?? "..").cnf?.jkt === ring.key.jkt);
  if (!issuerOnly && r.access_token) {
    const after = await web.api("GET", "/api/me", { key: ring.key, tokens: r });
    expect("... and it is good at the API", after.status === 200 && after.json?.user === email, brief(after));
  }
  const reuse = await web.refresh(rt1, { key: ring.key });
  expect("the OLD refresh token used again is invalid_grant", reuse.status === 400 && reuse.json?.error === "invalid_grant", brief(reuse));
  const dead = await web.refresh(r.refresh_token, { key: ring.key });
  expect("... and the NEW one is dead too: the whole session was revoked", dead.status === 400 && dead.json?.error === "invalid_grant", brief(dead));

  const bound = await web.login(email);
  const otherKey = await newKey();
  const wrongKey = await web.refresh(bound.tokens.refresh_token, { key: otherKey });
  expect("refresh with ANOTHER key is invalid_grant", wrongKey.status === 400 && wrongKey.json?.error === "invalid_grant", brief(wrongKey));
  const noKey = await web.refresh(bound.tokens.refresh_token, {});
  expect("refresh with no proof is invalid_grant", noKey.status === 400 && noKey.json?.error === "invalid_grant", brief(noKey));
  const still = await web.refresh(bound.tokens.refresh_token, { key: bound.key });
  expect("... neither used the token up: the right key still refreshes", still.status === 200 && still.json?.refresh_token, brief(still));

  const gone = await web.login(email);
  const wrongRevoke = await web.revoke(gone.tokens.refresh_token, { key: otherKey });
  expect("revoke with another key's proof is refused and revokes nothing", wrongRevoke.status === 400, brief(wrongRevoke));
  const revoked = await web.revoke(gone.tokens.refresh_token, { key: gone.key });
  expect("revoke (RFC 7009) with a proof of the key is 200", revoked.status === 200, brief(revoked));
  const afterRevoke = await web.refresh(gone.tokens.refresh_token, { key: gone.key });
  expect("a revoked refresh token is invalid_grant", afterRevoke.status === 400 && afterRevoke.json?.error === "invalid_grant", brief(afterRevoke));
  const unknown = await web.revoke("not-a-token", { key: gone.key });
  expect("revoking a token nobody knows is a 200 that does nothing (RFC 7009 section 2.2)", unknown.status === 200, brief(unknown));
  const out = await call("GET", `${web.toReach(doc.end_session_endpoint)}?${form({ post_logout_redirect_uri: `${origin}/`, state: "x", client_id: config.clientId })}`);
  expect("end_session redirects to the post-logout page, with the state", out.status === 302 && out.headers.get("location") === `${origin}/?state=x`, `${out.status} ${out.headers.get("location")}`);

  console.log(failed === 0 ? "browser-auth flow passed" : `browser-auth flow FAILED (${failed})`);
  process.exit(failed === 0 ? 0 : 1);
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main().catch((error) => {
    console.log(`FAIL ${error.stack ?? error}`);
    process.exit(1);
  });
}
