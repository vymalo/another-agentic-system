# The minimum application version

A deployment can refuse an application that is too old, so that a hard cutover (a change an older
application cannot follow) does not leave people on a build that fails in odd ways. The refused
application shows "update the application". This page is enough to implement both sides.

## The client

Every request of the resource API (`/api/...`, signed in) carries the header

```
X-Client-Version: 2.1.0
```

One to four decimal numbers separated by dots, each at most nine digits. `2.1` and `2.1.0` are the
same version. Anything else (`v2.1`, `2.1-beta`, an empty value) is not a version. A page of another
origin may send it: it is one of the request headers the CORS answer allows.

## The deployment

`server.minClientVersion` in the configuration ([config.md](config.md)), for example `2.1.0`.
**Absent (the default): no client is refused and the header is never read.**

When it is set, a request of the resource API is refused when its client states a version lower than
the minimum, states one that is not a version, or states none. The refusal comes before identity, so
an old application is told to update whether or not it is signed in. The health routes, the
public routes of a shared thread, the OAuth callback and the machine routes are not the
application's and are never asked.

## The problem

Status `426 Upgrade Required`, `application/problem+json`:

```json
{
  "type": "urn:vymalo:problem:client-update-required",
  "title": "Upgrade Required",
  "status": 426,
  "detail": "update the application: this deployment needs version 2.1.0 or later",
  "code": "client_update_required",
  "minClientVersion": "2.1.0"
}
```

`type` and `code` are stable: a client matches on either and shows its own "update the application"
screen. `detail` is for a person reading a log. `minClientVersion` is the lowest version served.
The client does not retry: the same request fails until the application is updated.

## Raising the minimum

A change an older application cannot follow raises `server.minClientVersion` in the same release
(the pull request template asks). Applications that do not yet send the header are refused as soon
as a minimum is set, so set it only when every build in use sends it.
