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
| [`roles-and-groups.json`](roles-and-groups.json) | the client roles `user` and `admin`, the groups `agentic-testers` (user) and `agentic-admins` (user and admin) | *Realm settings → Action → Partial import*, after the client exists |
| [`client-another-agentic-cli.json`](client-another-agentic-cli.json) | a public client for scripts: the device authorization grant only | *Clients → Import client* |

*Unverified:* that these files import as written. They follow the shape of a realm export (the partial import takes
`clients`, `roles` and `groups` in that format), but no Keycloak was run to try them. If an import is refused, make the same by hand from the list
below; the settings are what matters.

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
4. **Groups** `agentic-testers` (role `user`) and `agentic-admins` (`user` and `admin`). A person is **invited by being added to a group**.
   **Every account needs *Email verified* on**: the orchestrator refuses a token whose `email_verified` is `false` (ADR 0033). Set it when creating the user, or
   require *Verify Email* if the realm's SMTP works (*unverified*, like whether self-registration is on; it should be off).
5. **Client `another-agentic-cli`** (optional, for `dev/kc-token.sh` of a later PR and a CLI): public, *OAuth 2.0 Device Authorization Grant* on, standard flow off.
   Its access token carries `aud: another-agentic`, `email`, `email_verified` and `agentic_roles`, so a script gets a real token per test user without a password
   grant. oauth2-proxy skips a bearer token that verifies (`--skip-jwt-bearer-tokens`) and the orchestrator checks it again.

The user key is the **e-mail** (owner decision 4 of 2026-10-02): an e-mail changed in Keycloak orphans a person's threads. Tell the testers not to change it.
