/*
 * The minimum application version (docs/api/client-version.md): every request of the resource API
 * states this build's version in `X-Client-Version`, and a deployment that needs a newer one answers
 * 426 `client_update_required`. The app then shows "update the application" (`UpdateGate`) in place
 * of the page; the same request would fail again, so nothing is retried.
 */

/** One to four dotted numbers: this build's version, `package.json`'s unless the build names another. */
export const CLIENT_VERSION: string = process.env.NEXT_PUBLIC_CLIENT_VERSION ?? "0.1.0";

export const CLIENT_VERSION_HEADER = "X-Client-Version";
export const UPDATE_REQUIRED_CODE = "client_update_required";

export type UpdateNeed = { minClientVersion?: string };

let need: UpdateNeed | null = null;
const listeners = new Set<() => void>();

/** What the deployment asks for, or `null` while this build is served. */
export const updateNeed = (): UpdateNeed | null => need;

export function requireUpdate(next: UpdateNeed) {
  if (need !== null) return;
  need = next;
  for (const listener of [...listeners]) listener();
}

export function subscribeUpdateNeed(listener: () => void): () => void {
  listeners.add(listener);
  return () => void listeners.delete(listener);
}

/** Forgets the need, for tests. */
export function resetUpdateNeed() {
  need = null;
  listeners.clear();
}

/** The request with this build's version, for the API's own routes. */
export function withClientVersion(request: Request): Request {
  const headers = new Headers(request.headers);
  headers.set(CLIENT_VERSION_HEADER, CLIENT_VERSION);
  return new Request(request, { headers });
}

/** Notes the answer when it is the update-required problem (a clone is read, the caller keeps the body). */
export async function noteUpdateRequired(res: Response): Promise<void> {
  if (res.status !== 426) return;
  try {
    const body = (await res.clone().json()) as { code?: unknown; minClientVersion?: unknown };
    if (body.code !== UPDATE_REQUIRED_CODE) return;
    requireUpdate(
      typeof body.minClientVersion === "string" ? { minClientVersion: body.minClientVersion } : {},
    );
  } catch {
    // not a problem body: an ordinary 426 is the caller's error
  }
}
