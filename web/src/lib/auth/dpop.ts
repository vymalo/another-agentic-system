import { serverNowSeconds } from "./clock";

/*
 * A DPoP proof (RFC 9449) for a request to the orchestrator or the issuer's revocation endpoint:
 * `typ` `dpop+jwt`, ES256, the public key as `jwk`, and the claims `jti`, `htm`, `htu`, `iat` and,
 * for a request with an access token, `ath`. The token endpoint's proofs are `oauth4webapi`'s
 * (`oauth.DPoP`); this one signs the rest with the same key.
 */

const encoder = new TextEncoder();

export function base64url(bytes: ArrayBuffer | Uint8Array): string {
  const view = bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes);
  let text = "";
  for (const byte of view) text += String.fromCharCode(byte);
  return btoa(text).replaceAll("+", "-").replaceAll("/", "_").replaceAll("=", "");
}

const json = (value: unknown) => base64url(encoder.encode(JSON.stringify(value)));

/** `ath`: base64url of the SHA-256 of the access token. */
export async function accessTokenHash(accessToken: string): Promise<string> {
  return base64url(await crypto.subtle.digest("SHA-256", encoder.encode(accessToken)));
}

/** `htu`: the request's URL without query and fragment. */
export function htuOf(url: string, base?: string): string {
  const parsed = new URL(url, base);
  return `${parsed.origin}${parsed.pathname}`;
}

const publicKeys = new WeakMap<CryptoKey, Promise<JsonWebKey>>();

/** The public key as a JWK with only what RFC 7638 and RFC 9449 want (`kty`, `crv`, `x`, `y`). */
function publicJwk(key: CryptoKey): Promise<JsonWebKey> {
  let known = publicKeys.get(key);
  if (!known) {
    known = crypto.subtle.exportKey("jwk", key).then(({ kty, crv, x, y }) => ({ kty, crv, x, y }));
    publicKeys.set(key, known);
  }
  return known;
}

const randomId = (): string => base64url(crypto.getRandomValues(new Uint8Array(18)));

export type ProofInput = {
  method: string;
  url: string;
  accessToken?: string;
  nonce?: string;
};

export async function dpopProof(pair: CryptoKeyPair, input: ProofInput): Promise<string> {
  const header = { typ: "dpop+jwt", alg: "ES256", jwk: await publicJwk(pair.publicKey) };
  const payload: Record<string, unknown> = {
    jti: randomId(),
    htm: input.method.toUpperCase(),
    htu: htuOf(input.url),
    iat: serverNowSeconds(),
  };
  if (input.accessToken !== undefined) payload.ath = await accessTokenHash(input.accessToken);
  if (input.nonce !== undefined) payload.nonce = input.nonce;
  const signingInput = `${json(header)}.${json(payload)}`;
  // WebCrypto's ECDSA signature is r || s, which is exactly what a JWS ES256 signature is
  const signature = await crypto.subtle.sign(
    { name: "ECDSA", hash: "SHA-256" },
    pair.privateKey,
    encoder.encode(signingInput),
  );
  return `${signingInput}.${base64url(signature)}`;
}
