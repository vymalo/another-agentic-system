# orch-auth-header

The `Authenticator` over the identity header a proxy in front of the orchestrator sets: the behaviour
the orchestrator had before it was an OAuth2 resource server, as one implementation of the port.

## Where it sits

An **adapter** of the `Authenticator` port in [`orch-ports`](../ports/README.md)
([ADR 0009](../../../docs/decisions/0009-swappable-implementations-at-build-time.md),
[ADR 0033](../../../docs/decisions/0033-the-orchestrator-is-an-oauth2-resource-server.md)). It was
`orch-api`'s `auth.rs`; the API now asks the port. The binary
([`orchestrator`](../../bin/orchestrator/README.md)) depends on it behind the Cargo feature
`auth-header` (on by default) and builds it for `auth.mode: proxy_header` (the default) and
`jwt_or_proxy_header`. Depends on [`orch-core`](../core/README.md) and [`orch-ports`](../ports/README.md).

## What it does

- `HeaderAuth::new()` reads `Credentials::identity_header` (the API hands over `X-Auth-Request-Email`) and
  nothing else, **never the bearer token**. A value that is an e-mail address (non-empty, with an `@`, after
  trimming) is the user, lower-cased by `UserId::new`; the `Principal` has that e-mail, no name and no roles.
- A header that is present and is not an address is `AuthError::Invalid`, **even when a development user is
  configured**.
- No header: the development user (`HeaderAuth::with_dev_user`, `auth.devUser` / `AUTH_DEV_USER`) when there is
  one, else `AuthError::Missing` (fail closed). The dev user exists only in `proxy_header` mode: the
  configuration refuses it with `jwt` and `jwt_or_proxy_header`.
- It needs nothing outside itself: it is always ready and never `Unavailable`.

**The header is trustworthy only behind a proxy that strips the copies a client sends.** It cannot tell the
difference, so a deployment runs `jwt` and keeps this for one local user and for the one release of migration.

## Tests

- `tests/conformance.rs`: the `Authenticator` testkit (`orch_ports::authenticator_conformance!`) through an
  `AuthFixture`, plus: the normalised e-mail with no roles; the development user serves only a request without
  the header, and a garbled header is refused beside it; a bearer is not read.

## See also

[`orch-ports`](../ports/README.md), [`orch-auth-jwt`](../auth-jwt/README.md), [`orch-api`](../api/README.md).
