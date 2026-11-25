/*
 * The application's version, stated to the orchestrator and checked by it (`server.minClientVersion`, ADR 0076):
 * every call to the API carries `X-Client-Version`; a deployment that sets a minimum answers a call from a lower
 * version, or from one that states none, with the problem `client_update_required` (status 426), and the page then
 * shows the "update the application" screen (`features/session/components/update-required-screen.tsx`) instead of an
 * error line. With no minimum the header is never read, so sending it is harmless.
 */

/** The request header the version goes in. */
export const CLIENT_VERSION_HEADER = "X-Client-Version";

/** The `code` of the problem a client below the minimum gets. */
export const UPDATE_REQUIRED_CODE = "client_update_required";

/** This build's version: `package.json`'s, put in by `next.config.ts`. */
export const CLIENT_VERSION: string = process.env.NEXT_PUBLIC_CLIENT_VERSION ?? "0.0.0";

/** The request with its version stated; one that already states it is left alone. */
export function withClientVersion(request: Request): Request {
  if (request.headers.has(CLIENT_VERSION_HEADER)) return request;
  const headers = new Headers(request.headers);
  headers.set(CLIENT_VERSION_HEADER, CLIENT_VERSION);
  return new Request(request, { headers });
}

let required = false;
const listeners = new Set<() => void>();

/** Whether the deployment has refused this version: the page shows the update screen. */
export const updateRequired = (): boolean => required;

export function subscribeUpdateRequired(listener: () => void): () => void {
  listeners.add(listener);
  return () => void listeners.delete(listener);
}

/** Forgets it, for tests. */
export function resetUpdateRequired() {
  required = false;
  listeners.clear();
}

/** Whether the answer is the update-required problem: a 426 whose problem body says `client_update_required`. */
export async function isUpdateRequired(res: Response): Promise<boolean> {
  if (res.status !== 426) return false;
  try {
    const body: unknown = await res.clone().json();
    return (
      typeof body === "object" &&
      body !== null &&
      (body as { code?: unknown }).code === UPDATE_REQUIRED_CODE
    );
  } catch {
    return false;
  }
}

/** Reads an answer from the API: when it is the update-required problem, the update screen takes the page. */
export async function noteUpdateRequired(res: Response): Promise<void> {
  if (required || !(await isUpdateRequired(res))) return;
  required = true;
  for (const listener of [...listeners]) listener();
}
