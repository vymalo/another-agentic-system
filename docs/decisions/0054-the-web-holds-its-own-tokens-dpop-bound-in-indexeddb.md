# ADR 0054 — The web holds its own tokens: a public client, DPoP-bound, an offline refresh token in IndexedDB

- **Status:** accepted (2026-10-07), on the owner's words of 2026-10-07: "the frontend should use something to save the
  tokens locally, like e.g dexie.js or similar. Goal would be here to do offline token, ensuring that the refresh token is
  used", and their choice, the same day, of tokens held by the browser in IndexedDB (Dexie) over sessions kept by the edge
  (asked with the risk stated: a script that runs in the page can use what the page holds). The mitigations, names, routes
  and defaults below are the planner's and the owner may revisit them. Amends
  [ADR 0033](0033-the-orchestrator-is-an-oauth2-resource-server.md) (the orchestrator also takes DPoP-bound tokens; the
  edge no longer gates the web) and [ADR 0047](0047-one-ui-for-web-desktop-and-mobile-each-signs-in-as-a-public-oauth-client.md)
  (its recommendation that the browser web keep oauth2-proxy's cookie is replaced: the web is a public client like the
  native ones). Leaves [ADR 0040](0040-thread-sharing-by-revocable-link.md)'s public reader token-free.
  **Amended (2026-10-09)**, on the owner's words of that day ("add a proper login, not something 'deployed but hidden'"), and
  after the flow was run against a real Keycloak: the page never leaves for the issuer by itself (decision 2), the audience
  mapper of `another-agentic-web` writes the access token only (decision 1), and signing out is a control of the app
  (decision 11). See *Amendment (2026-10-09)* at the end.

## Context

Since ADR 0033 the browser never holds a token: oauth2-proxy keeps the session and the page has a cookie. In production
(`deploy/chart`, home-os overrides nothing) the session lives in the cookie (`oauth2Proxy.sessionStore: cookie`), and
behind Caddy's `forward_auth` a refreshed session never reaches the browser (`web/README.md`, "Why a 401 happens"), so
every request after `cookieRefresh` (10 minutes) redeems the same refresh token again and the person is signed out when
it rotates or the 12-hour cookie ends. oauth2-proxy asks for `openid email profile`, never `offline_access`. People are
signed out often, and the owner wants a refresh token that is really used and outlives the browser's session: an offline
token.

The owner chose that the browser keeps the tokens. A token in IndexedDB can be read by any script that runs in the
page, and an offline token lives for weeks. What limits that (facts *verified* 2026-10-07 against the sources named):

- **Keycloak 26.6.1** (home-os `charts/home-apps/keycloak-operator/kubernetes.yml`, the operator's default image) has
  DPoP (RFC 9449) GA and on by default (`Profile.java`, `DPOP`, `Type.DEFAULT`; release notes 26.4.0). A **public client
  with "Require DPoP bound tokens"** (`dpop.bound.access.tokens`) gets an access token **and a refresh token, offline
  included, bound to its key** (`TokenManager.java`: a public client's refresh token carries `cnf`; a refresh without a
  proof, or with another key's, is `invalid_grant`; guide `securing-apps/dpop`). Keycloak has **no `DPoP-Nonce`** at its
  token endpoint; it bounds a proof's `iat` to 10 s plus 15 s of skew and keeps `jti` single-use (`DPoPUtil.java`).
- `offline_access` is allowed for a public client with PKCE (`UserSessionManager.isOfflineTokenAllowed`). Offline Session
  Idle defaults to 30 days, Max to 60 days with the limit off by default; both can be shortened per client.
  **Revoke Refresh Token** applies to offline tokens ("you can use each offline token once only", `offline.adoc`), and a
  reuse ends that client session for both holders (`TokenManager.java`, `detachFromUserSession`).
- WebCrypto `CryptoKey` is `[Serializable]` and can be stored in IndexedDB; a non-extractable key cannot be exported
  (W3C WebCrypto, §5.2 and `exportKey`). `dexie@4.4.6` passes a `CryptoKey` through unchanged (`dist/dexie.mjs`,
  `intrinsicTypeNames`).
- **oauth2-proxy v7.15.5 has no DPoP**: `Authorization: DPoP …` never authenticates, and with `--skip-jwt-bearer-tokens`
  it would accept a DPoP-bound token sent as `Bearer` without checking `cnf`, which RFC 9449 §7.1 forbids
  (`pkg/middleware/jwt_session.go`). So DPoP requests must not go through oauth2-proxy.
- RFC 9449 §11.4: a script running in the page "can create new DPoP proofs as long as the client is online". DPoP stops
  the theft of a usable token, not a live takeover of the page; that is the content security policy's job.

## Decision

1. **The web is a public OAuth client of its own**, `another-agentic-web` (`deploy/keycloak/client-another-agentic-web.json`):
   standard flow with PKCE S256, no secret, **DPoP required** (`dpop.bound.access.tokens: true`), redirect
   `https://<host>/auth/callback`, web origin `https://<host>`, `offline_access` an optional scope, an access token of
   **5 minutes**, and the two mappers of the existing clients (audience `another-agentic`, `agentic_roles` from the
   `another-agentic` client's roles), so the orchestrator's audience and role checks are unchanged. The realm turns on
   **Revoke Refresh Token** with a reuse of 0. Both are the owner's to import and switch (`deploy/keycloak/README.md`).
2. **The web signs in itself** with `oauth4webapi` (MIT, panva; no storage of its own): Authorization Code with PKCE and
   the scope `openid email profile offline_access`, a full-page redirect to the issuer and back to `/auth/callback`; a
   sign-in again after the refresh token is refused opens the same flow in a popup that ends at `/auth/callback` and
   tells the page over `BroadcastChannel`, as the banner of today does.
3. **Everything is kept in IndexedDB through Dexie**, database `another-agentic-auth`:
   - `keys`: the DPoP key pair, ECDSA P-256, generated **non-extractable**, never leaving the browser;
   - `session`: the access token, its expiry, the refresh token (offline), and the claims the page needs (`sub`,
     `email`), one row per issuer and client;
   - `pending`: a sign-in under way (state, PKCE verifier, the page to return to), deleted at the callback and after
     10 minutes.
   Nothing of it goes to `localStorage`, a cookie or a log.
4. **The refresh token is used, once, by one tab.** The page refreshes when the access token has less than a minute left
   or a request meets a 401, inside a Web Lock (`navigator.locks`, `another-agentic.auth.refresh`) that first reads the
   row again: with rotation on, two tabs that redeemed the same refresh token would end the session. A refresh that is
   refused (`invalid_grant`) is "the session ended": the banner, the held requests and the person-switch check of
   `session-refresh.ts` stay as they are, with the token's `email` (else `sub`) as the person.
5. **Every request to the orchestrator is DPoP**: `Authorization: DPoP <access token>` and a `DPoP` proof (`typ`
   `dpop+jwt`, ES256, the public key in `jwk`, claims `jti`, `htm`, `htu` = the request's URL without query and fragment,
   `iat`, `ath`). The page corrects the proof's `iat` by its clock's offset from the server's `Date` header, as Keycloak
   allows only seconds of skew and no nonce.
6. **The orchestrator verifies DPoP itself** (`auth-jwt`, behind the `Authenticator` port, no new trait method), when
   `auth.dpop` is configured: one proof, `typ`, an allowed asymmetric `alg` (ES256, EdDSA), the signature by the header's
   public `jwk`, `htm` the request's method, `htu` one of `auth.dpop.publicOrigins` joined with the request's path, `iat`
   within 60 s past and 5 s ahead, `jti` not seen within that window (in memory: the chart runs one process; a split
   deployment accepts a replay across processes within the window, stated), `ath` the token's hash, and the token's
   `cnf.jkt` the thumbprint (RFC 7638) of the proof's key. **A token with `cnf` sent as `Bearer` is refused**, and so is
   a `DPoP` request whose token has none. A refusal is `401` with `WWW-Authenticate: DPoP error="invalid_dpop_proof"` or
   `error="invalid_token"`, `algs="ES256 EdDSA"`.
7. **The issuer the web uses is the orchestrator's to say**, at a public route `GET /api/public/auth` (outside identity,
   behind the public routes' rate limit): `{issuer, clientId, scope}` from `auth.browser`, a `404` when it is not
   configured. A `404` is the web of today: the edge's cookie, unchanged. This works in ADR 0047's static export, where
   the web has no server to read an environment variable from.
8. **The edge stops gating the web** when `auth.browser` is on (chart `auth.browser.enabled`): the web's pages and
   `/auth/callback` are served without `forward_auth`; a request to `/api` or `/agui` whose `Authorization` starts with
   `DPoP ` goes straight to the orchestrator with its `Authorization` and `DPoP` headers and without
   `X-Auth-Request-Email`; any other request goes through `forward_auth` as before, so a cookie of before keeps working
   until it ends. oauth2-proxy stays for that path; removing it is a later decision.
9. **Files are fetched, not linked.** `<img src>`, `<a download>` and "open" of a kept file cannot carry a header, so the
   web fetches the file with DPoP and shows an object URL; "open" shows an image in the page (an `<img>` never runs an
   SVG's script) and downloads anything else. A `blob:` URL has the page's origin and none of the server's CSP, so it
   is never opened as a page. The public reader keeps its plain links (no token).
10. **A content security policy on every page of the web**: `default-src 'self'`, `script-src 'self' 'nonce-<per
    request>' 'strict-dynamic'`, `connect-src 'self' <issuer origin>`, `img-src 'self' data: blob:`, `frame-ancestors
    'none'`, `base-uri 'none'`, `form-action 'self' <issuer origin>`, `object-src 'none'`, set by the web's server.
    ADR 0047's static export must keep an equal policy (hashes instead of a nonce) before it ships.
11. **Signing out revokes**: the refresh token is revoked at the issuer (RFC 7009, with a proof), the rows and the key
    are deleted, and the page goes to the issuer's end-session endpoint.

## Consequences

- A person stays signed in while they come back within the offline idle (30 days by default, the client's to shorten),
  across browser restarts, and independently of the Keycloak SSO session. Signing out of Keycloak elsewhere does not
  end an offline token; the account console's "Applications" or an administrator's revocation does.
- A stolen token is useless without the key, and the key cannot be read; a script running in the page can still act as
  the person while the page is open. The CSP and a 5-minute access token are what bound that.
- The orchestrator gains DPoP verification (about 200 lines over `jsonwebtoken`, which already parses `jwk` and
  computes RFC 7638 thumbprints) and a public route; `auth.dpop` and `auth.browser` are new configuration, both off by
  default. `jwt` mode without them is unchanged.
- The web gains `dexie` and `oauth4webapi`, `/auth/callback`, a CSP, and fetched files; `session-refresh.ts` keeps its
  shape with a token refresh where it pinged `/oauth2/userinfo`.
- `dev/mock-oidc` gains a public client, the `refresh_token` grant with rotation and reuse detection, `offline_access`,
  DPoP binding, revocation and CORS, so the flow is proven offline; the browser flow is proven by the web's Playwright
  specs on its mock server and by a compose scenario against the real orchestrator.
- The owner imports the client, sets `auth.browser.enabled` in home-os, and only after the last cookie session has ended
  (`oauth2Proxy.cookieExpire`, 12 hours) turns on Revoke Refresh Token: it is a realm setting, and oauth2-proxy with its
  session in the cookie redeems the same refresh token on every request, so rotation turned on earlier signs those people
  out. Until the client is imported and the value set, nothing changes in production.

## Amendment (2026-10-09)

- **The app's own sign-in screen** (decision 2). A person whom this browser holds no usable sign-in for sees the app's screen, the
  panda, a line naming the organisation (the Keycloak realm of `auth.browser`'s issuer, else its host) and one **Sign in** button,
  instead of being sent to the issuer when the page loads. The button starts the same authorization code flow with PKCE. The screen
  also stands in for the app, saying that the session has ended, when the issuer has refused the stored refresh token before the
  page had any use of it (a person coming back after it lapsed); a session that ends while the page is in use keeps the banner and
  its popup, which keep the page and its draft (`web/README.md`, "Signing in itself"). A share link that is not public, read by
  nobody signed in, is the same screen. Nothing changes with an edge.
- **The web's ID token carries no `another-agentic` audience** (decision 1). An ID token is not bound to the browser's key, and
  the orchestrator takes a token with `another-agentic` in `aud` as a plain `Bearer`; with the audience mapper on the ID token, the
  token the issuer returns beside the bound one would be a bearer credential for its five minutes. The mapper of
  `client-another-agentic-web.json` now writes the access token only (`id.token.claim: false`), and the import check
  (`deploy/keycloak/tests/import-check.sh`) asserts it. *Verified 2026-10-09* against Keycloak 26.6.1: before, the web's ID token
  was answered `200` by the orchestrator's `/api/me` as `Bearer`; after, `401`.
- **Signing out is the account menu's** (decision 11, unchanged in what it does): the menu at the foot of the sidebar, and the
  no-access screen, sign out with one click. `/auth/sign-out` stays, and still asks first (a page that signed a person out when
  opened could be opened by any other page). With an edge the same control goes to oauth2-proxy's `sign_out`, which the chart now
  gives `--backend-logout-url` so that Keycloak's session ends too (without it the start page signed the person straight back in:
  *verified 2026-10-09* with oauth2-proxy v7.15.5 and Keycloak 26.6.1).
- **What was run, 2026-10-09**: Keycloak 26.6.1 in a container (the realm made from `deploy/keycloak/` by its exports), the
  orchestrator of this repository with `auth.dpop` and `auth.browser`, the web's production build, and Chromium. Signing in
  from the screen, DPoP on every call, `/api/me` as the person, a refresh with a proof, an offline session at Keycloak, a reload
  that stays signed in, *Revoke Refresh Token* on (two rotations, two tabs and one refresh), a refresh token revoked at Keycloak
  (one refused refresh, the banner, no second attempt, the popup, the held request sent), a lapsed sign-in at load (the screen,
  "Your session has ended"), and signing out (the offline session revoked, Keycloak's session ended, back at the screen) all
  behaved as written. The client exports did not import as written before: two descriptions were longer than Keycloak's column
  (255), and the import answered 500; CI now imports them into Keycloak 26.6.1 (`deploy.yml`, `keycloak-import`).
