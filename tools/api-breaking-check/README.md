# api-breaking-check

Fails a pull request that breaks a client of [`docs/api/chat-api.yaml`](../../docs/api/chat-api.yaml). It
compares the file with the one on the base branch and lists, one per line, what a client would notice.
It is oasdiff's idea in the few rules this contract needs; it is not oasdiff.

```sh
npm --prefix tools/api-breaking-check ci                       # once per clone
node tools/api-breaking-check/check.mjs --base origin/main     # the working tree against a ref
node tools/api-breaking-check/check.mjs --old a.yaml --new b.yaml
npm --prefix tools/api-breaking-check test                     # the rules' own tests
```

Exit 0: nothing breaking. Exit 1: breaking changes. Exit 2: the check could not run.

## What counts as breaking

| Change | Why |
|---|---|
| a path or an operation removed; a `2xx` response removed; a response content type removed | a client calls or reads it |
| `operationId` changed | generated clients are named after it |
| a new required parameter; a parameter that became required; a request body that became required | an old client does not send it |
| a request property that became required, or a new required request property | same |
| a response property removed, or one that is no longer always present | a client reads it |
| a type or format changed (a request type may widen; a response type may not change) | a client parses it |
| a request enum value removed | an old client sends it |

Not flagged, on purpose: a new operation, a new optional field or parameter, a new response enum value (the
contract grows that way; whether clients must ignore unknown values is *unverified*: check the contract text
before relying on it), a removed error response. References (`$ref`) and `allOf` are followed; `oneOf` and
`anyOf` variants are not compared.

## A deliberate break

Put a `!` in the pull request title, for example `feat(api)!: ...`. The check then lists the breaks and
passes, so the break is visible in the log and in the title.

The CI job is `API breaking changes` in `.github/workflows/docs.yml` (pull requests only).
