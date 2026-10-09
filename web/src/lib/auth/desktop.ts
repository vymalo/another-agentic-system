/*
 * The desktop app's side of signing in (ADR 0047, decision 3; `apps/tauri/src-tauri/src/loopback.rs`): a listener on
 * 127.0.0.1 and a free port, the system browser opened at the authorization URL, and the address the issuer sent the
 * browser back to. The page keeps everything else (PKCE, the state, the code exchange with its own DPoP proof, the
 * tokens): the app's process never sees a token. Loaded only when the runtime configuration says `signIn: loopback`.
 */
import { runtimeConfig } from "@/lib/runtime-config";

/** Whether this build signs in through the system browser and a loopback listener (the desktop app). */
export const isLoopback = (): boolean => runtimeConfig().signIn === "loopback";

async function invoke<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  const { invoke: call } = await import("@tauri-apps/api/core");
  return call<T>(command, args);
}

/** A listener of the app on 127.0.0.1, and the redirect URI it answers (RFC 8252 section 7.3). */
export async function loopbackRedirect(): Promise<string> {
  const port = await invoke<number>("loopback_listen");
  return `http://127.0.0.1:${port}/callback`;
}

/** Opens `url` in the person's browser; resolves with the address the issuer sent it back to. */
export const loopbackAuthorize = (url: string): Promise<string> =>
  invoke<string>("loopback_sign_in", { url });

/** Opens `url` in the person's browser, never in the app (the issuer's end-session page). */
export const openInBrowser = (url: string): Promise<void> =>
  invoke<void>("open_in_browser", { url });
