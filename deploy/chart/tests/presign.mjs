// A SigV4 query presigner for S3, dependency-free (node:crypto), for tests/files-host-proof.sh. It signs what
// the orchestrator's S3 store signs with object_store 0.14.2 (orchestrator/crates/artifacts-s3): the query members
// X-Amz-Algorithm, -Credential, -Date, -Expires and -SignedHeaders, UNSIGNED-PAYLOAD, the host with its port, the extra
// query members and the signed headers it is given, and then X-Amz-Signature. It prints the URL.
//
//   node presign.mjs selftest
//   node presign.mjs <method> <https://host[:port]/path> <expiresSecs> [--query name=value]... [--header name=value]...
//
// The credentials are ACCESS_KEY and SECRET_KEY (default minioadmin, the dev stack's RustFS), REGION (us-east-1);
// NOW (an ISO time) pins the clock. `selftest` signs the example of AWS's documentation for a presigned GET (examplebucket, test.txt, 2013-05-24,
// 86400 s) and compares the signature with the value that example gives, which object_store's own test carries too:
// aeeed9bbccd4d02ee5c0109b86d86835f995330da4c265957d157751f604d404.
import { createHash, createHmac } from "node:crypto";

const enc = (s) => encodeURIComponent(s).replace(/[!'()*]/g, (c) => "%" + c.charCodeAt(0).toString(16).toUpperCase());
const hmac = (key, data) => createHmac("sha256", key).update(data).digest();
const sha = (data) => createHash("sha256").update(data).digest("hex");

export function presign({ method, url, expires, query = [], headers = [], accessKey, secretKey, region, now }) {
  const u = new URL(url);
  const stamp = now.toISOString().replace(/[-:]/g, "").replace(/\.\d+Z$/, "Z");
  const day = stamp.slice(0, 8);
  const scope = `${day}/${region}/s3/aws4_request`;
  const signed = new Map([["host", u.host]]);
  for (const [k, v] of headers) signed.set(k.toLowerCase(), v.trim());
  const names = [...signed.keys()].sort();
  const pairs = [
    ["X-Amz-Algorithm", "AWS4-HMAC-SHA256"],
    ["X-Amz-Credential", `${accessKey}/${scope}`],
    ["X-Amz-Date", stamp],
    ["X-Amz-Expires", String(expires)],
    ["X-Amz-SignedHeaders", names.join(";")],
    ...query,
  ];
  const canonicalQuery = pairs
    .map(([k, v]) => [enc(k), enc(v)])
    .sort((a, b) => (a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : a[1] < b[1] ? -1 : a[1] > b[1] ? 1 : 0))
    .map(([k, v]) => `${k}=${v}`)
    .join("&");
  // S3 encodes each path segment once.
  const canonicalPath = u.pathname.split("/").map((seg) => enc(decodeURIComponent(seg))).join("/");
  const canonicalHeaders = names.map((n) => `${n}:${signed.get(n)}\n`).join("");
  const canonicalRequest = [method, canonicalPath, canonicalQuery, canonicalHeaders, names.join(";"), "UNSIGNED-PAYLOAD"].join("\n");
  const toSign = ["AWS4-HMAC-SHA256", stamp, scope, sha(canonicalRequest)].join("\n");
  let key = hmac("AWS4" + secretKey, day);
  for (const part of [region, "s3", "aws4_request"]) key = hmac(key, part);
  const signature = hmac(key, toSign).toString("hex");
  return { url: `${u.origin}${canonicalPath}?${canonicalQuery}&X-Amz-Signature=${signature}`, signature };
}

const args = process.argv.slice(2);
if (args[0] === "selftest") {
  const { signature } = presign({
    method: "GET",
    url: "https://examplebucket.s3.amazonaws.com/test.txt",
    expires: 86400,
    accessKey: "AKIAIOSFODNN7EXAMPLE",
    secretKey: "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
    region: "us-east-1",
    now: new Date("2013-05-24T00:00:00Z"),
  });
  const want = "aeeed9bbccd4d02ee5c0109b86d86835f995330da4c265957d157751f604d404";
  if (signature !== want) {
    console.error(`selftest FAILED: ${signature} is not ${want}`);
    process.exit(1);
  }
  console.log("selftest ok: the signature of AWS's documented example");
} else {
  const [method, url, expires, ...rest] = args;
  if (!method || !url || !expires) {
    console.error("usage: presign.mjs <method> <url> <expiresSecs> [--query n=v]... [--header n=v]...");
    process.exit(2);
  }
  const query = [];
  const headers = [];
  for (let i = 0; i < rest.length; i += 2) {
    const [name, ...value] = rest[i + 1].split("=");
    (rest[i] === "--query" ? query : headers).push([name, value.join("=")]);
  }
  const { url: signedUrl } = presign({
    method,
    url,
    expires: Number(expires),
    query,
    headers,
    accessKey: process.env.ACCESS_KEY ?? "minioadmin",
    secretKey: process.env.SECRET_KEY ?? "minioadmin",
    region: process.env.REGION ?? "us-east-1",
    now: process.env.NOW ? new Date(process.env.NOW) : new Date(),
  });
  console.log(signedUrl);
}
