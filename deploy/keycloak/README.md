# Keycloak: the client, its roles and the groups

What the realm `vymalo` (`https://auth.verif.fyi/realms/vymalo`) needs for
[`deploy/chart`](../chart/README.md) ([ADR 0041](../../docs/decisions/0041-deployed-with-helm-on-kubernetes-secrets-by-externalsecret.md)).
It is made **by hand** in the admin console: the Keycloak operator's `KeycloakRealmImport` "only supports creation of new realms and does not
update" one (*verified 2026-10-03*, <https://www.keycloak.org/operator/realm-import>), and the realm exists. The JSON files are exports in
Keycloak's own format, to import or to read as a checklist. **None holds a secret**: the client's secret is made by Keycloak and goes
to AWS Secrets Manager (`oauth2_client_secret`), and CI fails if a file gets a secret-looking member.

| File | What | How to apply |
|---|---|---|
| [`client-another-agentic.json`](client-another-agentic.json) | the confidential client of oauth2-proxy | *Clients → Import client* |
| [`roles-and-groups.json`](roles-and-groups.json) | the client roles `user`, `admin` and, one per coder, `coder-vymalo` and `coder-stephane`; the groups `agentic-testers` (user), `agentic-admins` (user and admin), `agentic-coder-vymalo` (user and coder-vymalo) and `agentic-coder-stephane` (user and coder-stephane) | *Realm settings → Action → Partial import*, after the client exists |
| [`client-another-agentic-cli.json`](client-another-agentic-cli.json) | a public client for scripts: the device authorization grant only | *Clients → Import client* |
| [`client-another-agentic-web.json`](client-another-agentic-web.json) | a public client for the web itself: authorization code with PKCE, DPoP-bound tokens, an offline refresh token ([ADR 0054](../../docs/decisions/0054-the-web-holds-its-own-tokens-dpop-bound-in-indexeddb.md)) | *Clients → Import client*, then the two realm settings [below](#the-web-client-another-agentic-web-adr-0054) |
| [`client-another-agentic-desktop.json`](client-another-agentic-desktop.json) | a public client for the desktop app: the system browser and a loopback redirect, otherwise the web's ([ADR 0047](../../docs/decisions/0047-one-ui-for-web-desktop-and-mobile-each-signs-in-as-a-public-oauth-client.md)) | *Clients → Import client*, when the desktop app is used ([below](#the-desktop-client-another-agentic-desktop-adr-0047)) |

**They import as written** into Keycloak 26.6.1, the version of home-os's operator (*verified 2026-10-09*: [`tests/import-check.sh`](tests/import-check.sh)
makes a realm `vymalo` in a Keycloak container, imports the four clients and the partial import, reads back the web's and the desktop app's clients, and asks for the desktop app's loopback redirect on two ports; CI runs it, `deploy.yml`
job `keycloak-import`). Keep a client's `description` under 255 characters: Keycloak's column is that long, and a longer one makes *Import client* fail
with an unknown error (the first versions of the CLI's and the web's files did). The console labels below are from memory (*unverified*); the settings are
what matters.

## What each setting is for

1. **Client `another-agentic`**: OpenID Connect, *Client authentication* **on** (confidential), *Standard flow* on, *Direct access grants*
   **off**, *Implicit* off, PKCE method `S256` (oauth2-proxy sends `--code-challenge-method=S256`). *Valid redirect URIs*
   `https://agentic.servers.segning.pro/oauth2/callback`, *Web origins* `https://agentic.servers.segning.pro`, *Valid post logout
   redirect URIs* `https://agentic.servers.segning.pro/*`. *Access Token Lifespan* **15 minutes** (a client override): oauth2-proxy
   refreshes the session's tokens every `oauth2Proxy.cookieRefresh` (10 minutes), which must be shorter. *Unverified:* the realm's own
   default lifespan (Keycloak's is 5 minutes from memory), and that `keycloak-oidc` refresh re-issues an ID token that oauth2-proxy then forwards. The
   orchestrator ends a stream at the token's `exp` plus 60 seconds and the web reconnects (ADR 0033), so a 15-minute token is invisible to a person.
   After importing, **Credentials → Client secret** is the value of `oauth2_client_secret`.
2. **Client roles** `user` and `admin` on `another-agentic`. `user` is what lets a person in (oauth2-proxy's `--allowed-role=another-agentic:user`, and
   the orchestrator's `auth.roles.user`); `admin` is an operational name that **reads nobody's thread**. A person who holds neither is refused: the
   orchestrator's `defaultRole` is `null`.
   **One role per coder**, named like its agent (`coder-vymalo`, `coder-stephane`): the chart's `auth.roles` gives each `agents: [<that coder>]`, and a person reaches
   a coder when they hold its role **beside `user`** (the roles are unioned; `user` itself names only `chat` and `researcher` in that setup). A coder's role that
   `auth.roles` does not name grants nothing. A third coder is a third role here and in the chart's values ([`deploy/chart`, "Several coders"](../chart/README.md#several-coders-one-per-github-owner)).
3. **Two mappers on the client** (the plan's "client scope `another-agentic`" is these two mappers, put on each client so the import has no extra object):
   - *Audience*: *Included Client Audience* `another-agentic`, added to the ID token and the access token. The ID token's `aud` already holds the client id
     ([OpenID Connect Core §2](https://openid.net/specs/openid-connect-core-1_0.html), *verified 2026-10-03*), which is what
     `auth.jwt.audiences` checks; the mapper makes a **CLI client's access token** carry it too.
   - *User Client Role*: client `another-agentic`, token claim name **`agentic_roles`**, multivalued, in the ID token, the access token and userinfo. A flat claim
     keeps the orchestrator's `auth.jwt.rolesClaim` free of the client id. Keycloak does not put `realm_access` or `resource_access` in the **ID token** by
     default, and oauth2-proxy forwards the ID token, so this mapper is what carries the roles (*verified 2026-10-03*, a Keycloak 26.5.2 report,
     <https://github.com/9p4/jellyfin-plugin-sso/issues/346>; *unverified* for this realm's version).
   - The default scopes `email` (gives `email` and `email_verified`), `profile`, `roles` and `web-origins` stay. oauth2-proxy's `--allowed-role` reads the client role
     from the access token's `resource_access`, which the `roles` scope fills (*verified 2026-10-03*,
     <https://oauth2-proxy.github.io/oauth2-proxy/configuration/providers/keycloak_oidc>; the audience requirement is on that page too).
4. **Groups** `agentic-testers` (role `user`) and `agentic-admins` (`user` and `admin`), and one per coder, `agentic-coder-vymalo` (`user` and `coder-vymalo`) and `agentic-coder-stephane` (`user` and `coder-stephane`). A person is **invited by being added to a group**; a person who may use a coder is in `agentic-testers` or in that coder's group (which includes `user`), or in both coders' groups.
   **Every account needs *Email verified* on**: the orchestrator refuses a token whose `email_verified` is `false` (ADR 0033). Set it when creating the user, or
   require *Verify Email* if the realm's SMTP works (*unverified*, like whether self-registration is on; it should be off).
5. **Client `another-agentic-cli`** (optional, for `dev/kc-token.sh` of a later PR and a CLI): public, *OAuth 2.0 Device Authorization Grant* on, standard flow off.
   Its access token carries `aud: another-agentic`, `email`, `email_verified` and `agentic_roles`, so a script gets a real token per test user without a password
   grant. oauth2-proxy skips a bearer token that verifies (`--skip-jwt-bearer-tokens`) and the orchestrator checks it again.

The user key is the **e-mail** (owner decision 4 of 2026-10-02): an e-mail changed in Keycloak orphans a person's threads. Tell the testers not to change it.

## The web client `another-agentic-web` (ADR 0054)

[ADR 0054](../../docs/decisions/0054-the-web-holds-its-own-tokens-dpop-bound-in-indexeddb.md) makes the web a public OAuth client of its own: it signs in at
Keycloak itself, keeps its tokens in the browser's IndexedDB bound to a key the browser cannot export, and keeps an **offline** refresh token that it uses (once,
by one tab) to stay signed in. Nothing here is used until the chart's `auth.browser.enabled` is turned on ([`deploy/chart`](../chart/README.md#tokens-in-the-browser-adr-0054)):
importing the client changes nothing for the people who sign in through oauth2-proxy today. The owner does these steps, in this order, in the admin console of the realm
`vymalo` (the labels are those of Keycloak 26's console from memory, *unverified*: the settings are what matters):

1. **Import the client.** *Clients → Import client* → choose [`client-another-agentic-web.json`](client-another-agentic-web.json) → *Save*. It is **public** (*Client
   authentication* off: there is no secret to copy anywhere), *Standard flow* on and nothing else (*Direct access grants*, *Implicit*, *Service accounts* off), *Proof Key for Code
   Exchange Code Challenge Method* `S256`, ***Require DPoP bound tokens* on** (`dpop.bound.access.tokens`), *Valid redirect URIs*
   `https://agentic.servers.segning.pro/auth/callback`, *Web origins* `https://agentic.servers.segning.pro` (what lets the page call the token endpoint: CORS), *Valid post logout
   redirect URIs* `https://agentic.servers.segning.pro/*`, *Access Token Lifespan* **5 minutes**, and `offline_access` among the *optional* client scopes (the web asks for it).
   The two mappers are those of `another-agentic` and `another-agentic-cli` (the audience `another-agentic` and the claim `agentic_roles` from the
   client roles of `another-agentic`, so the orchestrator's audience and role checks do not change), with one difference: **the audience goes into the access
   token only**, not the ID token. The ID token is not bound to the browser's key, and the orchestrator would take one with `another-agentic` in `aud` as a
   plain `Bearer` (*verified 2026-10-09* against Keycloak 26.6.1: `200` before, `401` after; ADR 0054, *Amendment (2026-10-09)*).
2. **Optional: shorten how long an offline token lives.** Keycloak's *Offline Session Idle* defaults to 30 days and *Offline Session Max* to 60 days with the limit off;
   a client override is *Clients → another-agentic-web → Advanced → Advanced settings*, **Client Offline Session Idle** (and *Max*). The person stays signed in while they come back within the idle time.
3. **Check that the people have the realm role `offline_access`** (a realm's default roles hold it; *unverified* for `vymalo`): without it the token endpoint refuses the scope.
4. Then the chart: `auth.browser.enabled: true` in home-os.
5. **Last, at least 12 hours later, turn on Revoke Refresh Token** (*Realm settings → Tokens*): *Revoke Refresh Token* **on** and *Refresh Token Max Reuse* **0**. A refresh
   token, an offline one included, is then good once; the web redeems it in one tab at a time, and a second use of the same token (a stolen copy) ends that client session
   for both holders. **This is a realm setting: it applies to every client of the realm, `another-agentic` (oauth2-proxy) included**, and oauth2-proxy with its session
   in the cookie redeems the same refresh token again and again (`deploy/chart/values.yaml`, `oauth2Proxy.sessionStore`): turned on while people still sign in through
   oauth2-proxy, it signs them out. After step 4 nobody does, once their cookie has ended (`oauth2Proxy.cookieExpire`, 12 hours). Until this step the web works the same,
   without the protection against a reused token.

**What this does not do.** Signing out of Keycloak elsewhere does **not** end an offline token; the person's account console (*Applications*) or an administrator (*Users → Consents* /
*Sessions → Offline*) does. A script that runs in the page can still use the person's session while the page is open (DPoP stops the theft of a usable token, not a live takeover): the web's
content security policy and the 5-minute access token are what bound that (ADR 0054, *Consequences*).

**Verified 2026-10-07** against the sources the ADR names (*Context*, which carries the details): Keycloak 26.6.1 has DPoP (RFC 9449) GA and on by default; a public client with *Require DPoP bound
tokens* gets an access token **and a refresh token, offline included, bound to its key** (`TokenManager.java`; a refresh without a proof or with another key's is `invalid_grant`; guide `securing-apps/dpop`);
Keycloak has **no `DPoP-Nonce`** at its token endpoint and bounds a proof's `iat` to 10 s plus 15 s of skew with a single-use `jti` (`DPoPUtil.java`); `offline_access` is allowed for a public client with PKCE
(`UserSessionManager.isOfflineTokenAllowed`); *Revoke Refresh Token* applies to offline tokens (`offline.adoc`).

**Run 2026-10-09** against Keycloak 26.6.1 in a container, with the realm made from these files, the orchestrator and the web's production build in
Chromium (ADR 0054, *Amendment (2026-10-09)*, lists every step): sign-in from the app's screen, DPoP on every call, a refresh with a proof, an offline session,
*Revoke Refresh Token* on (rotation, two tabs and one refresh), a revoked refresh token (the banner, no second attempt, the popup), and sign-out (the offline
session revoked, Keycloak's session ended) behaved as written. Keycloak's discovery answers any origin, and its token and revocation endpoints let the page
send the `DPoP` header (`Cors.DEFAULT_ALLOW_HEADERS` of 26.6.1 lists it, *verified 2026-10-09* in the source) to the client's *Web origins*. *Unverified:*
the console labels above.

## The desktop client `another-agentic-desktop` (ADR 0047)

The desktop app ([`apps/tauri`](../../apps/tauri/README.md)) is the web's page in a window, so its client is the web's with two differences
([ADR 0047, *Amendment (2026-10-09): the desktop app*](../../docs/decisions/0047-one-ui-for-web-desktop-and-mobile-each-signs-in-as-a-public-oauth-client.md#amendment-2026-10-09-the-desktop-app)):

- *Valid redirect URIs* is **`http://127.0.0.1/callback`**: the app opens the person's browser and listens for the redirect on a port of the loopback
  address that the system gives it. Keycloak matches a `127.0.0.1` redirect on any port ([RFC 8252, section 7.3](https://www.rfc-editor.org/rfc/rfc8252#section-7.3)),
  and refuses `localhost` or another path (*verified 2026-10-09* against Keycloak 26.6.1, `tests/import-check.sh`).
- *Web origins* are the webview's own: `tauri://localhost` (macOS, Linux), `http://tauri.localhost` (Windows, Android) and `https://tauri.localhost`, which let the
  page call the token and revocation endpoints (*verified 2026-10-09*: Keycloak answers a preflight from `tauri://localhost` with that origin). There is
  no *Valid post logout redirect URI*: signing out opens Keycloak's page in the browser and stays there.

Import it when the desktop app is used (*Clients → Import client*); the realm settings of the web's client (the realm role `offline_access`, *Revoke Refresh
Token*) hold for it too. The API must also let the webview's origins call it: the chart's `orchestrator.cors.allowedOrigins`
([`deploy/chart`, "Calls from the desktop app"](../chart/README.md#calls-from-the-desktop-app)).
