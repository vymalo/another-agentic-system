# ADR 0047 — One UI for web, desktop and mobile; each signs in as a public OAuth client

- **Status:** proposed (2026-10-06). **Accepted** on the owner's direction of 2026-10-06 (*"we're packing the UI into tauri for
  building a desktop and a mobile application… start thinking about auth in such cases (in app browser for mobile, server +
  callback for desktop, simple redirect for web)"*): one UI, packed into Tauri for desktop and mobile, with those three sign-ins.
  **Proposed**, for the owner to confirm: everything else, which is the static build and what it changes, the browser web's
  keeping the edge, the redirect URIs, the token storage, the CORS settings and the lifecycle. **Nothing of this is built.**
  Extends [ADR 0033](0033-the-orchestrator-is-an-oauth2-resource-server.md); amends nothing, but [ADR 0045](0045-admin-dashboard-in-the-web-and-agent-access-from-the-registry.md)
  gets a dated note of today (its route handler cannot exist in a static build). *Amended 2026-10-07 by [ADR 0054](0054-the-web-holds-its-own-tokens-dpop-bound-in-indexeddb.md):* the browser web no longer keeps oauth2-proxy's cookie; it is a public client like the native ones, its tokens DPoP-bound in IndexedDB. The static export must keep 0054's content security policy (hashes instead of a nonce). *Amended 2026-10-09:* decision 1 and the CORS of decision 2 are built,
  with the choices *Amendment (2026-10-09): the static export* records at the end; the desktop app is built as *Amendment (2026-10-09): the desktop
  app* records, which **replaces the OS keychain of decision 3's table** with ADR 0054's DPoP-bound tokens in the webview's IndexedDB.

## Context

Today the web is `output: "standalone"` (`web/next.config.ts`): a Node server behind oauth2-proxy, a session cookie, and `/api`
and `/agui` on the web's own origin, routed by the edge ([ADR 0033](0033-the-orchestrator-is-an-oauth2-resource-server.md),
[ADR 0041](0041-deployed-with-helm-on-kubernetes-secrets-by-externalsecret.md)). Tauri cannot run a Node server, so the web
must be a static export: `output: 'export'` allows no dynamic routes without `generateStaticParams()`, no cookies, rewrites,
redirects, headers, proxy (middleware), request-dependent route handlers or Server Actions, and no default image loader
(*verified 2026-10-06*, <https://nextjs.org/docs/app/guides/static-exports>).

The orchestrator is already an OAuth2 resource server: it validates a JWT bearer against the issuer's JWKS (`iss`, `aud` among
`auth.jwt.audiences`, `exp`) and maps the roles claim ([ADR 0033](0033-the-orchestrator-is-an-oauth2-resource-server.md)). A
native client only has to send one.

## Decision

### 1. The web is a static SPA build, served by a static server and packed unchanged into Tauri

`output: 'export'`; the web image serves `out/` with a small static server (*proposed*: Caddy, as the edge already is) instead of
`node server.js`; the same `out/` is the Tauri frontend. **What in `web/` must change** (found by reading it, 2026-10-06; no
code is changed here):

| In `web/` | Why it cannot stay | Becomes |
|---|---|---|
| `next.config.ts` `output: "standalone"` | needs `node server.js` | `output: 'export'` |
| `next.config.ts` `rewrites()` (`/api`, `/agui`, `/oauth2` to the mock or a real orchestrator, dev and e2e only) | unsupported in export, also in `next dev` | the mock server (`web/mock/server.ts`) serves `out/` and the API on one origin for the e2e; `next dev` calls the API origin directly (needs the CORS of decision 3) |
| `next.config.ts` `headers()` (`nosniff`, `Referrer-Policy: same-origin`) | unsupported in export | the static server sets them; the `Referrer-Policy` keeps the share token out of referrers ([ADR 0040](0040-thread-sharing-by-revocable-link.md)) |
| `src/app/threads/[id]/page.tsx` and `src/app/s/[token]/page.tsx`: dynamic segments read with `await params` | ids are not known at build; no `generateStaticParams()` | one exported shell per area that reads the id from the path on the client; the static server answers any `/threads/*` and `/s/*` with it. Whether Tauri's asset protocol falls back the same way is *unverified*; if not, the app navigates by a query (`?t=<id>`) |
| `notFound()` in `threads/[id]/page.tsx` (a non-UUID id) | request-time | the check moves to the client component, which draws `not-found.tsx` |
| `src/app/manifest.ts` (a metadata route) | must be static in export | keep it static (`force-static`, to be checked at build); the manifest means nothing in Tauri |
| `NEXT_PUBLIC_SIGN_IN_PATH` and a same-origin `baseUrl: ""` in `src/lib/api/client.ts`, `credentials: "same-origin"` in `files.ts` and `session-refresh.ts` | inlined at build, one build per deployment; assume the edge's origin and cookie | a runtime `config.json` (*proposed*: API origin, issuer, client id, sign-in mode) read at start, so one image or one app build serves a deployment |
| `<img src={file.href}>` and download links (`kept-file-card.tsx`, `sources-tab.tsx`) | a header cannot ride on `src`, only the cookie | in bearer mode the file is fetched with the header and shown from a blob URL; in edge mode as today |
| `/admin/api/[...path]` route handler of [ADR 0045](0045-admin-dashboard-in-the-web-and-agent-access-from-the-registry.md) (not built) | a server | gone; see the note of today in that ADR |

Not found in `web/src` (grep, 2026-10-06): `next/headers`, `cookies()`, `headers()`, `middleware` or `proxy`, route handlers,
`"use server"`, `next/server`, `next/image`, `revalidate`, `generateStaticParams`. `images.unoptimized` is already set.

### 2. Every client calls the orchestrator directly with a bearer

Native clients (and, later, the Platform API) are called with `Authorization: Bearer <JWT>`: the **access token**, with
`aud` including `another-agentic` and the `agentic_roles` claim, from the audience and role mappers the CLI client already has
([`deploy/keycloak/`](../../deploy/keycloak/README.md)). No change to the orchestrator's check.

- **AG-UI streaming.** The web does **not** use `EventSource` today: `ThreadAgent` (`src/features/chat/lib/agui/thread-agent.ts`)
  sends `fetch` with `Accept: text/event-stream` and reads the body with its own `readSse` (`sse.ts`, which keeps the `id:` line
  for `Last-Event-ID`). So the stream needs only the header added by the one `fetch` seam that `withSessionRefresh` wraps today
  (`client.ts`, `thread-agent.ts`). The orchestrator ends a stream at the token's `exp` plus 60 s ([ADR 0033](0033-the-orchestrator-is-an-oauth2-resource-server.md));
  the client then refreshes the token and reconnects with `Last-Event-ID`, as it reconnects today.
  Streaming `fetch` in the iOS and Android webviews is *unverified*; the fallback is Tauri's HTTP plugin.
- **CORS** (a layer on the orchestrator, *proposed* setting `server.cors.allowedOrigins`): an allow-list, never `*`; allowed
  headers `Authorization`, `Content-Type`, `Accept`, `Last-Event-ID`; no credentials (no cookie is sent cross-origin). The list is the
  deployment's origins plus the Tauri origins, *unverified* strings: `tauri://localhost` (macOS, Linux, iOS), `http://tauri.localhost`
  (Windows, Android), and `https://tauri.localhost` where Tauri's `useHttpsScheme` is on. The browser web on the edge's own origin
  needs none. A bad origin is refused, as the surface's other refusals ([`api/agui.md`](../api/agui.md)).
- **The browser web keeps oauth2-proxy and the cookie session** *(recommended)*. Its tokens never reach JavaScript (no XSS
  theft of a refresh token), its calls stay same-origin (no CORS, no preflight), and the keep-warm, refresh-on-401 and sign-in
  popup of `web/README.md` "Signing in again" stay as built and proven. The cost is two transports in one client: **`edge`**
  (cookie, same origin, today's code) and **`bearer`** (native), behind one seam that every request already passes through.
  oauth2-proxy **still does**: the browser's sign-in and callback, the cookie, the forwarded ID token, the `--allowed-role` gate,
  and the pass-through of a verified bearer from a native client (`--skip-jwt-bearer-tokens`, with an `--extra-jwt-issuers`
  `issuer=audience` pair if the audience differs; *unverified* for these tokens). **SPA PKCE for the browser too** would give one
  code path and let the edge drop oauth2-proxy, but it puts a refresh token in the page; revisit it if the edge is ever
  simplified.

### 3. Sign-in per platform: a public client with PKCE (RFC 8252)

All three use the authorization code flow with PKCE `S256` ([RFC 7636](https://www.rfc-editor.org/rfc/rfc7636),
[RFC 8252](https://www.rfc-editor.org/rfc/rfc8252): an external user agent or an in-app browser tab, never an embedded webview
that could read the password), no client secret, `state` checked.

| Platform | Flow | Callback | Token storage |
|---|---|---|---|
| Web (browser) | redirect through the edge (oauth2-proxy, confidential client `another-agentic`) | `https://<host>/oauth2/callback` | the edge's cookie; nothing in JavaScript |
| Desktop (Tauri) | the **system browser** and a **loopback** listener | `http://127.0.0.1:<random port>/callback`, e.g. with `tauri-plugin-oauth` (<https://www.lib.rs/crates/tauri-plugin-oauth>; maintenance *unverified*) | OS keychain (refresh token); access token in memory |
| Mobile (Tauri) | an **in-app browser tab**: `ASWebAuthenticationSession` (iOS), Custom Tabs (Android) | an app-claimed https link (Universal Links, App Links) or, failing that, a custom scheme (`com.vymalo.agentic:/oauth2/callback`, *proposed* name) | Keychain (iOS), Keystore (Android) |

- **Keycloak: one public client per platform** (*proposed* ids `another-agentic-desktop`, `another-agentic-ios`,
  `another-agentic-android`), *Client authentication* off, *Standard flow* on, PKCE `S256` required, direct access grants off, each
  with only its redirect URIs, the audience and roles mappers, and a refresh-token revocation setting (below). A loopback redirect
  with any port (`http://127.0.0.1:*`) is the RFC 8252 shape; whether Keycloak matches it so is *unverified*. Https app links need the
  static server to serve `/.well-known/apple-app-site-association` and `/.well-known/assetlinks.json`. **`deploy/keycloak/` gains these
  client exports when the apps are built**, not before.
- **Refresh-token rotation.** Each refresh token is used once and the new one stored (Keycloak's *Revoke Refresh Token*, *unverified* here, so a replayed token is refused). Access tokens stay in memory and are
  renewed ahead of `exp`; a refresh that fails is a sign-in again, never a loop.
- **Sign-out revokes**: the client revokes the refresh token at the issuer (RFC 7009 `revocation_endpoint`), ends the SSO session
  (`end_session_endpoint`, in the same browser tab as the sign-in), then deletes the keychain entry. Signing out only locally is not
  a sign-out.
- **MCP OAuth callbacks stay on the orchestrator** ([ADR 0046](0046-people-add-their-own-mcp-servers-secrets-in-a-credential-broker.md)):
  they are the orchestrator's and the broker's redirect URIs at the third-party server, and no app is involved in them. One thing to
  carry over: the browser that reaches `GET /api/user-tool-servers/oauth/callback` is **not** the app's session on desktop and
  mobile, so that route must identify the person by `state` alone (bound at `begin_oauth`, used once), never by a cookie or bearer, and
  end on a page that sends the person back to the app. ADR 0046 carries this rule in its amendment of the same day.

```mermaid
sequenceDiagram
  participant U as Person
  participant C as Client (web, desktop or mobile)
  participant B as Browser (edge, system browser or in-app tab)
  participant K as Keycloak
  participant O as Orchestrator
  alt Web in a browser
    C->>B: GET /oauth2/start (the edge)
    B->>K: authorize (the edge's client, PKCE)
    K-->>B: redirect to /oauth2/callback
    B-->>C: cookie session
    C->>O: call with the cookie (the edge adds the bearer)
  else Desktop
    C->>C: start a loopback listener on 127.0.0.1 and a random port, make verifier and state
    C->>B: open the system browser at authorize (S256 challenge)
    B->>K: sign in
    K-->>B: redirect to http://127.0.0.1:port/callback with code and state
    B-->>C: the listener reads code and state, closes
    C->>K: token request (code, verifier)
    K-->>C: access and refresh tokens
    C->>C: refresh token to the OS keychain
    C->>O: call with Authorization Bearer
  else Mobile
    C->>B: open ASWebAuthenticationSession or a Custom Tab at authorize (S256 challenge)
    B->>K: sign in
    K-->>B: redirect to the app link or custom scheme with code and state
    B-->>C: the OS hands the URL to the app, the tab closes
    C->>K: token request (code, verifier)
    K-->>C: access and refresh tokens
    C->>C: refresh token to Keychain or Keystore
    C->>O: call with Authorization Bearer
  end
  U->>C: sign out
  C->>K: revoke the refresh token and end the session
  C->>C: delete the stored token
```

```mermaid
stateDiagram-v2
  [*] --> signed_out
  signed_out --> signing_in: sign in (verifier, state)
  signing_in --> signed_in: code exchanged, tokens stored
  signing_in --> signed_out: refused, cancelled or state mismatch
  signed_in --> signed_in: access token renewed with the refresh token (rotated)
  signed_in --> signed_out: refresh refused (expired, revoked or replayed)
  signed_in --> signed_out: sign out (revoked at the issuer, store cleared)
```

## Consequences

- **One build, one image, one app.** The web image stops being a Node image (smaller, no `node_modules` at runtime); e2e and dev
  serve `out/` from the mock or from the static server. Every spec that depends on `next start` or on `rewrites()` moves with it.
- **Invariants.** 3 holds: nothing new is stored by the orchestrator, tokens live on the client. 1 and 7 hold: Tauri is a shell for
  the UI, not an agent host. Fail-closed holds: a wrong origin, audience or token is refused.
- **New on the orchestrator**: the CORS layer and its setting (`config.md`, its schema and the chart's values change when built);
  the `auth.jwt.audiences` already lists `another-agentic`.
- **A second transport in the web**, tested in both modes against `dev/mock-oidc/` (which needs a public client with a loopback
  redirect), and the web's README, `architecture.md` and DESIGN notes change in the slice that builds it.
- **Not decided here**: Tauri project layout, signing and distribution, and the Platform API's own origin and CORS (the platform's
  AD-026, amended in parallel).

## Alternatives rejected

- **A web server (BFF) inside every app**: Tauri has none, and a local one would be a second session store.
- **An embedded webview sign-in** (the form inside the app): RFC 8252 forbids it for good reason, the app would see the password.
- **A custom scheme on every platform**: any app can claim it; an app-claimed https link cannot be taken by another app, so it is
  preferred on mobile, the scheme being the fallback.
- **One shared public client for all apps**: one redirect list for three platforms, no way to revoke one app, no per-platform setting.
- **Keep Next.js server rendering and ship a thin native shell around the hosted site**: no offline start, no keychain, and a store
  review of a site wrapper is the likelier rejection.

## Amendment (2026-10-09): the static export

Built on the owner's "start with the tauri too" of 2026-10-09. What decision 1 left *proposed* or *unverified* is now this:

- **`output: 'export'`** for every build; `next dev` keeps its rewrites to the mock or an orchestrator (`web/next.config.ts`). With an export,
  `distDir` is where the pages go (Next 16.3.6 `build/index.js`, `hasCustomExportOutput`, *verified 2026-10-09*), so the e2e's three builds export
  side by side (`out`, `out-session`, `out-browser`).
- **One page per kind of address**, `/threads/_` and `/s/_`, exported with `generateStaticParams` and `dynamicParams = false`. The static server
  answers any `/threads/<id>` with it, and the data Next fetches when the app moves there: `<id>.txt` and the per-segment `<id>/__next.*.txt`
  (Next 16.3.6 appends `.txt` in export mode and accepts `text/plain` as a flight answer, `fetch-server-response.js`; segment files by
  `addSegmentPathToUrlInOutputExportMode`, *verified 2026-10-09* in the source and by `e2e/static-export.spec.ts`: moving between threads loads no
  page). The page reads the id from the address after its first render, which must equal the exported one. **Tauri falls back otherwise**: an asset
  it does not have is `<path>.html`, then `<path>/index.html`, then `index.html` (`crates/tauri/src/manager/mod.rs` `get_asset` at `tauri-v2.12.1`,
  *verified 2026-10-09*), so the desktop app maps `/threads/<id>` to the shell itself, wrapping its assets with `Context::set_assets` (the next slice).
- **The content security policy is two halves**, because a static page cannot have a nonce made per request: the static server's header (everything
  but the hashes; `script-src 'self' 'unsafe-inline'`; the issuer from `WEB_CSP_CONNECT_SRC`) and, first in every page's `<head>`, a meta written
  after the build with `script-src 'self'` and the SHA-256 of each inline script of that page. A browser enforces both (CSP3) and a hash voids
  `'unsafe-inline'` in its policy (CSP2), so only the build's own inline scripts run; `'strict-dynamic'` is gone, `'self'` covers Next's chunks.
  This is ADR 0054 decision 10's "an equal policy (hashes instead of a nonce)". The desktop app has no header: its policy is Tauri's, beside the meta.
- **The static server is Caddy 2.11.4** in the web image (`web/Caddyfile`): the shells, `nosniff`, `Referrer-Policy: same-origin`, the header half,
  `/config.json` never cached, the 404 page with the same headers. The image is no longer Node: `USER 1000`, read-only, `/tmp` its only writable place,
  and **`NET_BIND_SERVICE` must stay** in a pod that drops every capability, because the caddy binary carries that file capability and exec fails
  without it (found 2026-10-09 by running the image so; the chart's edge already kept it).
- **`/config.json`** (`web/src/lib/runtime-config.ts`), read once per page load: `apiOrigin`, `clientId`, `signIn` (`redirect` or `loopback`) and
  `organisation`, each optional; the image ships `{}` (the API on the page's own origin, the issuer and client from `GET /api/public/auth`). The issuer
  and the scope always come from the orchestrator, whose tokens they are.
- **CORS** (decision 2): `server.cors.allowedOrigins`, exact origins of any scheme (the apps' `tauri://localhost`, `http://tauri.localhost`), never
  `*` or `null`, no credentials ever; allowed request headers `Authorization`, `DPoP`, `Content-Type`, `Accept`, `Last-Event-ID`, `X-Web-Revision`;
  exposed `WWW-Authenticate` (the page reads a DPoP refusal from it), `Date` (its clock) and `Content-Disposition`. A preflight is answered before
  identity. The chart's `orchestrator.cors.allowedOrigins` writes it, only in browser mode, and lets a preflight through the edge without oauth2-proxy.

## Amendment (2026-10-09): the desktop app

Built the same day, desktop first (`apps/tauri/`, the layout this ADR left open):

- **The app is Tauri 2** (`tauri` 2.12.1, `tauri-build` 2.7.1, `tauri-plugin-opener` 2.7.0, `@tauri-apps/cli` and `@tauri-apps/api` 2.12.1, all
  pinned exactly, released 2026-09-29 to 2026-09-30, *verified 2026-10-09* on crates.io and npm). It packs the web's export (`web/out-tauri`,
  built by `apps/tauri/scripts/build-web.mjs` with a `config.json` of `apiOrigin`, `clientId: another-agentic-desktop` and `signIn: loopback`)
  and maps `/threads/<id>` and `/s/<token>` to their shells by wrapping its embedded assets (`Context::set_assets`, which hands back the
  previous provider "so you can use it as a fallback", *verified 2026-10-09* in `crates/tauri/src/lib.rs` at `tauri-v2.12.1`).
- **The tokens stay where ADR 0054 keeps them**, in the page: a non-extractable ECDSA key and the DPoP-bound access and offline refresh tokens in
  IndexedDB, in the webview's profile, refreshed once under a Web Lock. **Not the OS keychain** this ADR's table proposed. The reason is concrete:
  the code that signs in, refreshes, proves and recovers (the banner, the held requests, the person switch, revocation) is the browser web's, built
  and run against Keycloak 26.6.1 (ADR 0054, *Amendment (2026-10-09)*); a keychain would need a second token store in Rust, a channel to hand the
  page an access token on every refresh, and the proof made outside the page or the key exported to it. DPoP already makes a copied token useless
  without the key, and the key cannot be read, by a script or by anything with the profile's files. The cost: the refresh token is in the
  webview's storage, which another program of the same user could read (as it could a browser's); with a keychain it would need that user's
  unlocked keychain. Revisit with mobile, where the platform stores (Keychain, Keystore) are the norm.
- **Sign-in is the system browser and a loopback listener the app writes itself** (`apps/tauri/src-tauri/src/loopback.rs`, about a hundred lines):
  `127.0.0.1` on a port the system picks, one `GET /callback` answered, anything else refused, five minutes at most; `tauri-plugin-oauth`
  (the proposal) is not used, so no third-party code sees the callback. The page makes PKCE S256, the `state` and the URL, exchanges the code with
  its own proof, and keeps the pending sign-in's redirect URI to use it again at the token endpoint; then it loads itself again where it was, as
  a browser's page does after its callback (the banner's sign-in keeps the page, as the popup does). **Keycloak 26.6.1 matches a registered
  `http://127.0.0.1/callback` with any port** (RFC 8252 section 7.3) and refuses `localhost` and another path; it allows the token endpoint to the
  client's web origins `tauri://localhost` and `http://tauri.localhost`, and its discovery to any origin (*verified 2026-10-09* against a Keycloak
  26.6.1 container: this ADR's two *unverified* points). The client is `deploy/keycloak/client-another-agentic-desktop.json`.
- **Sign-out** revokes from the page and opens the issuer's end-session page in the browser, never in the app, with no `post_logout_redirect_uri`
  (no page of the app is registered at the issuer).
- **CORS**: the chart's `orchestrator.cors.allowedOrigins: [tauri://localhost, http://tauri.localhost]` (amendment above); the app's own policy
  (`tauri.conf.json`) names the API's and the issuer's origins in `connect-src`; Tauri sends it with the hashes of each page's inline scripts
  added (`set_csp` in `crates/tauri/src/manager/mod.rs` at `tauri-v2.12.1`, *verified 2026-10-09*), beside the export's own meta of the same hashes.
- **CI** (`.github/workflows/desktop.yml`) builds the Linux `.deb` and AppImage, unsigned, and keeps them as an artifact. **Mobile is not
  scaffolded**: it needs the Android SDK and NDK in CI and a macOS runner for iOS; `apps/tauri/README.md`, "Not yet", lists what the next slice needs.
