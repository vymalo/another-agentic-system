/**
 * A UUIDv7 (RFC 9562, section 5.7): 48 bits of Unix time in milliseconds, then random bits.
 *
 * The consumer mints the id of the thread its first message creates (AG-UI, ADR 0012), and the
 * orchestrator lists threads by id, newest first: only a time-ordered id keeps the list in the
 * order the threads were made. `crypto.randomUUID()` is a v4, random, and would shuffle it.
 */
export function uuidv7(
  now: number = Date.now(),
  fill: (bytes: Uint8Array) => Uint8Array = (b) => crypto.getRandomValues(b),
): string {
  const bytes = fill(new Uint8Array(16));
  let ms = now;
  for (let i = 5; i >= 0; i--) {
    bytes[i] = ms % 256;
    ms = Math.floor(ms / 256);
  }
  bytes[6] = 0x70 | ((bytes[6] ?? 0) & 0x0f); // version 7
  bytes[8] = 0x80 | ((bytes[8] ?? 0) & 0x3f); // variant 10
  const hex = Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("");
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
}
