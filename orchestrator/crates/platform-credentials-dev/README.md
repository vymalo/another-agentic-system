# orch-platform-credentials-dev

The `PlatformCredentials` of a development stack: a token from the development issuer
(`mock-oidc`), asked for with a `client_credentials` request, the way `dev/auth-header.sh` does.
**Not for a deployment**: `provisioner.credentials.dev` is refused when `server.environment` is
`production`.

| Item | Is |
|---|---|
| `DevConfig` | the issuer's URL, the client id and secret, an optional audience, the timeout; `Debug` hides the secret |
| `DevCredentials::new(cfg)` | builds the adapter; nothing is sent until a token is asked for |
| `PlatformCredentials::token()` | `POST <url>/token` with the client as HTTP basic and `grant_type=client_credentials`; the `access_token` of the answer. Asked for on every call, nothing is cached |

Errors: `Refused` for `401` and `403`, `Unavailable` for another status or a connection that
fails, `Invalid` for an answer with no `access_token`. No error or `Debug` holds the secret, a URL
or a token. No redirect is followed.

Tests: `cargo test -p orch-platform-credentials-dev` (an in-process stand-in of `/token`).
