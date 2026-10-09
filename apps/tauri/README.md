# Desktop app

The chat in a window of its own ([ADR 0047](../../docs/decisions/0047-one-ui-for-web-desktop-and-mobile-each-signs-in-as-a-public-oauth-client.md)):
the web's static export ([`web/README.md`, "Static export"](../../web/README.md#static-export)) packed into a [Tauri 2](https://v2.tauri.app/)
app. It is the same UI, the same API and the same tokens as the browser web in browser mode
([ADR 0054](../../docs/decisions/0054-the-web-holds-its-own-tokens-dpop-bound-in-indexeddb.md)); only the sign-in and the address
of the API differ. Linux is built in CI; macOS and Windows build from the same sources (*unverified*: not run).

| Path | What |
|---|---|
| `deployment.json` | the deployment the app is built for, in one place: `apiOrigin` (`https://agentic.servers.segning.pro`) and `issuerOrigin` (`https://auth.verif.fyi`); `AGENTIC_API_ORIGIN` and `AGENTIC_ISSUER_ORIGIN` override them |
| `scripts/build-web.mjs` | builds the web's export into `web/out-tauri`, writes its `config.json` (`apiOrigin`, `clientId`: `AGENTIC_CLIENT_ID`, default `another-agentic-desktop`, `signIn: loopback`) and the policy's `connect-src` for those two origins (`src-tauri/tauri.deployment.json`, passed to `tauri build --config`) |
| `src-tauri/build.rs` | the issuer's origin from the same file (or variable), compiled in: the one address the app opens in the browser |
| `src-tauri/tauri.conf.json` | the window, the bundle, and the content security policy without a remote origin (the build adds the two) |
| `src-tauri/src/assets.rs` | `/threads/<id>` and `/s/<token>` answered with their shells, as the web image's Caddy does (Tauri would fall back to `index.html`) |
| `src-tauri/src/loopback.rs` | the sign-in's listener on `127.0.0.1` and the system browser (below) |
| `src-tauri/icons/` | made from `web/public/brand/icon-512.png` with `pnpm tauri icon` |

## Build

```sh
sudo apt-get install libwebkit2gtk-4.1-dev librsvg2-dev libxdo-dev libssl-dev   # Linux, once (Tauri's prerequisites)
cd apps/tauri
pnpm install
pnpm build:debug        # the web for the app, then a debug .deb: src-tauri/target/debug/bundle/deb/
pnpm build              # release .deb and AppImage (what CI keeps: .github/workflows/desktop.yml)
cd src-tauri && cargo test --locked
```

For another deployment, edit `deployment.json`, or: `AGENTIC_API_ORIGIN=https://chat.example.com AGENTIC_ISSUER_ORIGIN=https://id.example.com pnpm build`.
Build with `pnpm build` (or `build:debug`): a bare `tauri build` has no `connect-src` for the API.

## What the deployment needs

1. The Keycloak client [`deploy/keycloak/client-another-agentic-desktop.json`](../../deploy/keycloak/client-another-agentic-desktop.json): public, PKCE S256,
   DPoP-bound tokens, the redirect `http://127.0.0.1/callback` (Keycloak matches it with any port: RFC 8252 section 7.3) and the web origins
   `tauri://localhost`, `http://tauri.localhost` and `https://tauri.localhost` (what lets the page call the token endpoint).
2. The chart in browser mode, with `orchestrator.cors.allowedOrigins: [tauri://localhost, http://tauri.localhost]`
   ([`deploy/chart/README.md`, "Calls from the desktop app"](../../deploy/chart/README.md#calls-from-the-desktop-app)), and an orchestrator image
   that has `server.cors`.

## Signing in

```mermaid
sequenceDiagram
  participant P as Page (webview)
  participant A as App (Rust)
  participant B as System browser
  participant K as Keycloak
  participant O as Orchestrator
  P->>A: loopback_listen
  A-->>P: a port on 127.0.0.1
  P->>P: PKCE verifier, state, redirect http://127.0.0.1:port/callback (kept in IndexedDB)
  P->>A: loopback_sign_in(authorization URL)
  A->>B: open the URL
  B->>K: sign in
  K-->>B: redirect to 127.0.0.1:port/callback with code and state
  B->>A: GET /callback
  A-->>B: "You can go back to the app"
  A-->>P: the callback address
  P->>K: token request with the code, the verifier and a DPoP proof (CORS: tauri://localhost)
  K-->>P: access and offline refresh tokens bound to the page's key
  P->>P: loads again at the page it was on
  P->>O: DPoP requests to the API's origin (CORS)
```

```mermaid
stateDiagram-v2
  [*] --> signed_out
  signed_out --> listening: Sign in (loopback_listen)
  listening --> in_browser: loopback_sign_in opens the browser
  in_browser --> signed_in: callback, state checked, code exchanged
  in_browser --> signed_out: refused, abandoned (5 minutes) or a state the page did not make
  signed_in --> signed_in: refresh (rotated), once, under a Web Lock
  signed_in --> signed_out: Sign out (revoked; the issuer's page opens in the browser)
```

Once signed in, the page loads itself again where it was, as a browser's page does after the callback (what asked for a session before there was
one waits for that load); a sign-in from the banner of a session that ended keeps the page, as the browser's popup does. While the browser is open, the sign-in screen says so and has **Cancel** (`loopback_cancel`): the listener closes and the page is as before, with no
error, so an abandoned browser tab does not hold the button for five minutes. The page keeps what it keeps in a browser: a non-extractable DPoP key and the tokens in IndexedDB, in the webview's own profile. The app's
process makes no request to the issuer and never sees a token; it only listens on the loopback address for one `GET /callback`, which it
refuses for any other path or method (a connection that sends nothing for 5 seconds, fails or asks for something else is dropped and the listener
goes on), and opens in the browser only an address of the issuer's origin it was built with. The `state` and PKCE stop a forged
callback from another local process. Signing out revokes the refresh token from the page and opens the issuer's end-session page in the browser,
never in the app. Why the webview's IndexedDB and not the OS keychain: [ADR 0047, Amendment (2026-10-09): the desktop app](../../docs/decisions/0047-one-ui-for-web-desktop-and-mobile-each-signs-in-as-a-public-oauth-client.md#amendment-2026-10-09-the-desktop-app).

## Tests

`cargo test --locked` in `src-tauri`: the shells' mapping (`assets.rs`: a thread, its data and its segment files to `/threads/_`, a share link to
`/s/_`, everything else itself), and the listener's (`loopback.rs`: only a `GET /callback` is the callback; only an address of the build's issuer is
opened, never another host, port or scheme; a silent, closed or other connection does not hold up the callback; the first of two futures wins, as
a cancel does). The page's side is `web/src/lib/auth/desktop.test.ts` (the loopback redirect, the code exchanged with a proof, the page loaded again or
kept, the issuer's `access_denied` and a state the page did not make each storing nothing and exchanging nothing, signing out in the browser) and
`web/src/features/session/components/sign-in-screen.dom.test.tsx` (Cancel while the browser is open, and no error after it).

**The app against the web's mock**, by hand (run 2026-10-09 on Linux under Xvfb, with `curl` as the browser): the mock is the issuer and the API
(`web/README.md`, the mock's browser mode) with the loopback redirect and the webview's CORS switched on, and the app is built for it:

```sh
cd web && MOCK_BROWSER_AUTH=1 MOCK_LOOPBACK=1 MOCK_PUBLIC_ORIGINS=tauri://localhost,http://127.0.0.1:4010 \
  MOCK_CORS_ORIGINS=tauri://localhost pnpm mock &
cd apps/tauri && AGENTIC_API_ORIGIN=http://127.0.0.1:4010 AGENTIC_ISSUER_ORIGIN=http://127.0.0.1:4010 AGENTIC_CLIENT_ID=another-agentic-web \
  sh -c 'pnpm build:web && pnpm tauri build --debug --no-bundle --config src-tauri/tauri.deployment.json'
src-tauri/target/debug/another-agentic
```

The app shows the sign-in screen; **Sign in** opens the browser at the mock's authorization endpoint, which sends it back to the app's listener;
the app then shows the chat, and `GET /__mock/issuer?session=default` counts one code grant and the API's requests, none refused. With a browser
that never comes back, the screen says to finish in the browser and **Cancel** brings it back with no error (run again 2026-10-09 with these commands).

## Not yet

- **Signing and distribution**: the bundles are unsigned, and nothing is published (no updater, no store).
- **Mobile** (ADR 0047: an in-app browser tab, app links): not scaffolded. The next slice needs `tauri android init` and `tauri ios init`, the
  Android SDK and NDK in CI (iOS needs a macOS runner and an Apple team), a sign-in through `ASWebAuthenticationSession` and Custom Tabs
  with a claimed https link or the custom scheme `com.vymalo.agentic:/oauth2/callback`, the static server's `/.well-known/assetlinks.json` and
  `apple-app-site-association`, Keycloak clients `another-agentic-ios` and `another-agentic-android`, and the Android origin
  `http://tauri.localhost` in `server.cors.allowedOrigins`. Whether streaming `fetch` works in the mobile webviews is *unverified*.
- **Choosing the deployment** at the first start: the API's origin is fixed when the app is built.
