# orch-artifacts-s3

`ArtifactStore` over an S3 bucket: AWS S3 or any server that speaks its API (MinIO, Ceph, Cloudflare R2, and so on).
The production store.

## Where it sits

An **adapter** of the `ArtifactStore` port in [`orch-ports`](../ports/README.md)
([ADR 0032](../../../docs/decisions/0032-files-from-agents-live-in-an-artifact-store.md),
[ADR 0009](../../../docs/decisions/0009-swappable-implementations-at-build-time.md)): the files an agent hands over are
kept here by the hash of their content, and the event log keeps only the reference. It is a thin layer over the S3
backend of the [`object_store`](https://crates.io/crates/object_store) crate (0.14.2, Apache Arrow's; *verified
2026-10-02* on crates.io and by reading its manifest). Only the binary ([`orchestrator`](../../bin/orchestrator/README.md))
depends on it, behind the Cargo feature `artifacts-s3` (on by default), and builds it from `artifacts.s3.*` when
`artifacts.store` is `s3`. The store for development and one node is [`orch-artifacts-fs`](../artifacts-fs/README.md).

## Build cost

`object_store` is built with `default-features = false, features = ["aws"]` (workspace `Cargo.toml`): the S3 backend,
and none of the local file system, Azure, Google Cloud or plain HTTP. It uses the workspace's `reqwest` 0.13 and
`aws-lc-rs`, the TLS backend this process already has, so there is still one TLS stack. The lock file gains six crates:
`object_store`, `crc-fast`, `humantime`, `itertools` 0.15, `quick-xml` 0.41 and `spin` 0.10 (`hyper`, `base64` 0.23, `rand`
0.10, `md-5`, `chrono` and the rest were in it already). A cold `cargo check -p orch-artifacts-s3` took 14 seconds on top of
a built workspace. A build that does not want it takes the feature off (`--no-default-features`, then the features it
wants), and a configuration that asks for `store: s3` then exits 78.

## API at a glance

| Item | What |
|---|---|
| `S3Artifacts::new(S3Config) -> Result<S3Artifacts, BuildError>` | builds the store; it does not connect (a control plane starts without waiting for the bucket, and an unreachable bucket is found by the first call). Installs the `rustls` crypto provider if none is installed. Cheap to clone |
| `S3Config::new(bucket)` | `.with_region(r)` (default `us-east-1`, what compatible servers expect), `.with_endpoint(url)` (a server other than AWS: the bucket is then in the path, `<endpoint>/<bucket>/<key>`; `http://` is allowed), `.with_public_endpoint(url)` (the host of every **presigned** URL: the one agents reach the bucket at, when it is not the Service this process reaches it at; without one nothing is presigned), `.with_prefix(p)` (keys go under `p/`; one bucket for several deployments), `.with_credentials(access_key_id, secret_access_key)` (`SecretString`s; **required**), `.with_timeout(d)` (one request, default 60 s). `Debug` shows `<redacted>` for the credentials |
| `BuildError::Config(text)` | no credentials, or a configuration the library refuses; the text never holds a credential |
| `impl ArtifactStore for S3Artifacts` | `put`, `get`, `delete`, `copy` and `delete_prefix` as the port says, and the optional calls of [ADR 0058](../../../docs/decisions/0058-large-files-go-up-through-a-presigned-url-the-orchestrator-gives-the-agent.md): `can_presign` (a public endpoint is set), `presign_put`, `presign_get`, `head` and `read_prefix`, below |

## How a file is kept

* One object per file, `[<prefix>/]threads/<thread uuid>/<sha256>`. The **media type is the object's `Content-Type`**; the hash
  is user metadata `x-amz-meta-sha256`, and the file name `x-amz-meta-filename`, **percent-encoded** because a metadata
  value is ASCII (a file with no name has no such header). The size is the object's own. No sidecar object: a put is one
  `PUT`, atomic on the server's side, so a file is either whole with its meta or not there.
* **Idempotent by content**: a put again overwrites the object with the same bytes (and the meta of the second call).
* `get` reads the object as a stream. An object whose `x-amz-meta-sha256` names another hash (`ArtifactError::another_hash()`, which `head` answers the same, and which a caller can tell from the others), that has no content type or
  whose file name is not UTF-8 is `ArtifactError::Corrupt` (never served); an object somebody else wrote with a content type
  and no user metadata reads fine, with no file name.
* `delete` is one `DELETE` (the bulk call is turned off: not every compatible server has it). A key that is not there is fine.
* `delete_prefix(thread)` (the erasure of a deleted thread's files, [ADR 0043](../../../docs/decisions/0043-deleting-a-thread-erases-it.md)) **lists** `[<prefix>/]threads/<thread uuid>/` (`ListObjectsV2`, followed through its continuation tokens; the listing is by whole path segments, so no other thread's and no other prefix's objects) and deletes each object with its own `DELETE`, twenty at a time (the bulk `DeleteObjects` stays off, as for `delete`). It returns how many objects it deleted; a thread with none is `Ok(0)`. The library reports a refused *listing* as a failed request, so a refusal there is `Unavailable` and not `Unauthenticated`; the purge that retries it does not depend on the class. Verified against `tests/support` and against RustFS 1.0.1-preview.17 through the testkit (2026-10-09).
* `copy` is one **server-side `CopyObject`** (`x-amz-copy-source`, no body): the bytes stay in the bucket and the object's content type and user metadata go with it. The key is the content's hash, so there is nothing to re-hash. A missing source is `NotFound`; a copy of a key onto itself (S3 refuses that) is a `HEAD`; two keys of different hashes are `Invalid` and nothing is sent. Verified against `tests/support` and against RustFS 1.0.1-preview.17 through the testkit (2026-10-09).
* **Path-style addressing with an endpoint**: the bucket is in the path, `<endpoint>/<bucket>/<key>`, which every compatible server
  takes (RustFS serves only that unless it is given server domains). **Without one** (AWS) the bucket is the host name,
  `https://<bucket>.s3.<region>.amazonaws.com/<key>` (`with_virtual_hosted_style_request(true)`), so a bucket name with a dot fails
  AWS's TLS certificate there (the chart refuses one). **No request checksum**: no
  `x-amz-checksum-*` header is sent (no checksum algorithm is set, and the bulk delete, which would add one, is off); the payload is
  signed (`x-amz-content-sha256`). That is what an AWS SDK does with `AWS_REQUEST_CHECKSUM_CALCULATION=when_required`; this store does
  not read that variable and needs none.
* **Credentials** are the two secrets of the configuration, used as given (static, SigV4). This store does not read `AWS_*`
  variables, the instance profile or a web-identity token; a deployment that needs those needs a change to this crate (not planned: the
  configuration gives both keys, by reference). They are in no error and no `Debug`.
* **Errors.** A refusal (HTTP 401 or 403) is `Unauthenticated`, permanent. Everything else the server or the network does is
  `Unavailable`, transient, with the library's error as its source (it holds the URL, so it never reaches the chat log).
  `object_store` retries a failed request twice within 30 seconds, then the caller sees the error and decides.

## Presigned uploads (`uploads/v1`) and downloads (A1)

* **Two clients, one bucket** ([ADR 0058](../../../docs/decisions/0058-large-files-go-up-through-a-presigned-url-the-orchestrator-gives-the-agent.md), A2).
  SigV4 signs the `Host`, so a URL for the Service the orchestrator reaches the bucket at (`http://release-rustfs:9000`) does not
  work at the name an agent reaches (`https://agentic-files.servers.segning.pro`). With `with_public_endpoint(url)` the store
  builds a second `object_store` client for the same bucket and credentials at that URL and **uses it to sign and for nothing
  else**: no request goes to the public host from here. Every call that touches the bucket (`put`, `get`, `head`, `copy`,
  `delete`, `read_prefix`) goes to the internal endpoint. Signing is local, so it never fails for a bucket that is down.
* **`presign_put(key, meta, ttl)`** signs a `PUT` for `[<prefix>/]threads/<uuid>/<hash>` with `object_store`'s
  `Signer::signed_url_opts` (read in its `src/signer.rs`, 0.14.2) and folds **four headers into the signature**, which the answer lists
  and the request must send with exactly these values: `x-amz-checksum-sha256` (the declared hash, base64: the form S3 defines,
  *verified* by `object_store`'s own test, `uU0nuZNNPgilLlLX2n2r+sSE7+N6U4DukIj3rOLvzek=` for `hello world`), `content-type` (the
  declared type), and the two user metadata a plain `put` writes, `x-amz-meta-sha256` and (when the file has a name)
  `x-amz-meta-filename`, percent-encoded. So a request with other bytes (the server checks the body against the checksum), another
  type or another key is refused, and the object an agent leaves **reads back like one this store put** (`get` gives the same
  meta, name included). The lifetime is `ttl` (`X-Amz-Expires`), one second to seven days; a meta that disagrees with the key is
  `Invalid`. Without a public endpoint it is `Unsupported`.
* **`presign_get(key, overrides, ttl)`** (downloads by a redirect, ADR 0058 A1) signs a `GET` with the **public** signer. The `response-cache-control`,
  `response-content-disposition` and `response-content-type` that are set are **query members of the signature** (`SignedUrlOptions::with_query`, whose pairs are encoded with `%20` for a space, never `+`), and the only signed
  header is `host`, so the browser sends nothing but the URL and a `Range` is allowed. S3 takes these members only on a signed request, so the holder of the URL cannot change, remove or add one. The lifetime is whole
  seconds from one to seven days (S3 refuses `X-Amz-Expires=0`; under a second is `Invalid`), `expires_at` is the floor of now, in whole seconds, plus that (`X-Amz-Date` is floored to the second, so it is never later than the signature's own end), the overrides are checked first (`ResponseOverrides::check`), and signing is local. Without a public endpoint it
  is `Unsupported`. The host in the URL is the public one, so the bucket is the first path segment and the prefix follows.
* **`head(key)`** is one `HEAD` signed (in the query) for the **internal** endpoint with the header that asks for the checksum,
  `x-amz-checksum-mode: ENABLED`, sent with `reqwest` because `object_store` has no call that returns it (it does not parse the
  checksum of a `HEAD`). **Every attempt is signed afresh** (a signature lives a minute and an attempt may take the whole timeout, so
  the retry of a slow one never sends a signature that is about to run out). It vouches for what `get` does: an object whose
  `x-amz-meta-sha256` names **another hash than its key** is `Corrupt` (one with no such metadata, put by someone else, is fine). It reads the size (`Content-Length`, required), the type, the time (`Last-Modified`; absent or not a date it is `None`, "unknown", never a default, and the ingest reads it as late once the URL has stopped working) and, **when the server says
  it**, `x-amz-checksum-sha256` (only if `x-amz-checksum-type` is absent or `FULL_OBJECT`). A server that does not say it gives
  `sha256: None`, which the ingest reads as "hash the object yourself". `404` is `None`; `401` and `403` are `Unauthenticated`; a
  5xx or a transport error is `Unavailable`, tried three times (250 ms, then 500 ms) and shown **without the URL** (it carries
  the signature). Redirects are not followed.
* **`read_prefix(key, len)`** is one ranged `GET` (`Range: bytes=0-<len-1>`). An empty object, for which a server answers a range with
  an error, is an empty prefix; a missing one is `None`.

## Tests

* `tests/s3.rs`, **always**: the `ArtifactStore` testkit (twenty cases) over real HTTP against `tests/support`, a small
  in-process S3 (path style, in-memory, `CopyObject`, `ListObjectsV2` in pages of ten, checks the access key id of the signature header, answers S3's XML errors, can be
  told to fail); and the cases of this adapter: a put is one `PUT` to `/<bucket>/<prefix>/threads/<uuid>/<hash>` signed
  with the key, with `Content-Type`, `x-amz-meta-sha256` and a percent-encoded `x-amz-meta-filename` (`Résumé 100%.svg`
  becomes `R%C3%A9sum%C3%A9%20100%25%2Esvg`); no name header for a file with no name; an object written by someone else
  reads with what it has; three kinds of object that are not what their key says are `Corrupt`; wrong credentials are
  `Unauthenticated` for put, get and delete and are shown in no error and no `Debug`; a server that answers 500 is transient,
  the error shows no credential and no address, the retries are bounded and the store recovers; a port nobody listens on is
  transient; a put refused before it is sent sends nothing; a copy is one `PUT` with `x-amz-copy-source` and no bytes, keeps the content type and the metadata, and leaves the copy when the source is deleted; a copy of a missing file is `NotFound`, of a file onto itself a `HEAD`; a copy refused for its credentials is `Unauthenticated`, one that meets a failing server is transient with no credential shown, and one to another hash sends nothing; an erasure of a thread lists its own part under the prefix (three pages for 25 keys), deletes each object with one `DELETE` and no bulk call, touches no other thread's objects and no other prefix's, and is `Ok(0)` the second time; one that meets a refused key or a failing server removes nothing and shows no credential; building does not connect and needs credentials.
* The same twenty cases against **an S3-compatible server**, when `ORCH_TEST_S3_URL` names one (otherwise they print
  `skipped: no S3-compatible server` and pass, as the Postgres tests do without `ORCH_TEST_DATABASE_URL`).
  `ORCH_TEST_S3_BUCKET` (default `orch-test`; it must exist), `ORCH_TEST_S3_REGION` (`us-east-1`), `ORCH_TEST_S3_ACCESS_KEY`
  and `ORCH_TEST_S3_SECRET_KEY` (`minioadmin`) complete it, and each run writes under a prefix of its own. For example,
  with the RustFS of the repository's `compose.yaml` (profile `s3`):

  ```sh
  docker compose --profile s3 run --rm rustfs-bucket   # starts RustFS, waits until it is ready, creates the bucket orch-test
  ORCH_TEST_S3_URL=http://127.0.0.1:9000 cargo test -p orch-artifacts-s3   # its credentials are the defaults above
  ```

  Run 2026-10-09 against RustFS 1.0.1-preview.17 (the pin of `compose.yaml`): the twenty cases, the stub's and the unit tests, 56 in all,
  passing. CI's `check` job (`.github/workflows/orchestrator.yml`) starts the same RustFS as a service, creates the bucket and sets the
  variables, so `cargo test --workspace` runs them. (The first run, 2026-10-02, was against moto 5.2.3's S3 server.)
* **Presigned uploads**, over the stub and, with `ORCH_TEST_S3_URL`, over the server (the URL is also the public endpoint):
  the `artifact_presign_conformance!` suite (the declared bytes land and read back, `head` finds them, other bytes of another length and
  of the same length are refused or never vouched for, a slot agrees with its key), on the stub in three ways (a server that enforces
  the signed checksum as S3 does, one that does not and stores the checksum it computed, one that never says a checksum) and on the server;
  `artifact_no_presign_conformance!` for a store with no public endpoint; and the cases of this adapter: a slot is signed for the **public**
  host (`https://agentic-files.example.com`) with the bucket first in the path, the prefix and the key after it, `X-Amz-Expires`, and
  `X-Amz-SignedHeaders` exactly the headers of the answer plus `host`, the checksum is the base64 of the SHA-256, the name is percent-encoded,
  a file with no name signs no name, **signing sent nothing to anybody**, and the URL is shown by no `Debug`; the object an agent leaves reads
  back with the same meta as a put; the stub, which refuses a presigned request that lacks any header it signed (`403`) and a body that is not
  the signed checksum (`400`), refuses each in turn and takes the declared bytes; a `HEAD` is one request signed in the query (no
  `Authorization`) with `x-amz-checksum-mode`, and reads the size, type, time and checksum; a server that says no checksum gives `None`, a
  missing file `None`; a `HEAD` with wrong credentials is `Unauthenticated`, one that meets a failing server is transient after three
  tries and shows no signature, one to a port nobody listens on is transient and its error chain shows no signature either;
  `read_prefix` is one `GET` with `Range: bytes=0-9`; a store with no public endpoint signs nothing and sends nothing. Against the server the run also prints what it
  says of a checksum (`what_the_server_says_of_a_checksum_is_the_right_one_or_nothing`). *Unverified until CI has run it:* that RustFS 1.0.1-preview.17
  refuses a presigned `PUT` whose body is not the signed checksum and says the checksum on a `HEAD`; the suite is written so that
  either answer passes as long as the store never vouches for bytes that are not the declared ones, and it fails if RustFS stores a
  signed header it did not check and reports it.
* **Migrating a directory store into this one** ([ADR 0058](../../../docs/decisions/0058-large-files-go-up-through-a-presigned-url-the-orchestrator-gives-the-agent.md) A5.4, [`orch_artifacts_fs::migrate`](../artifacts-fs/README.md#moving-a-directory-into-another-store); `orch-artifacts-fs` is a dev-dependency here), the module `migrating_a_directory`, over the stub (always) and, with `ORCH_TEST_S3_URL`, over the server (its objects under a prefix of their own):
  a volume of five files (two threads, a name that is not ASCII, an empty file, one of 3 MiB, one whose bytes are not its key's) is dry-run (nothing is written), migrated (four objects, each reading back with the meta of the directory, as a plain put writes it; the damaged file named and not uploaded) and migrated again (the four
  skipped, which on a store that says no checksum for a plain put means a read-back); against a stub that says its checksums as well; the object of a file with a name that is not ASCII carries the percent-encoded `x-amz-meta-filename` and the hash; a stub that answers every request `403` fails the files one by one and `Unauthenticated` shows no credential.
  *Run 2026-10-14 against RustFS 1.0.1-preview.17, the pin of `compose.yaml`.*
* **What a real server holds a presigned `PUT` to**, only with `ORCH_TEST_S3_URL` (the module `a_real_server_holds_a_presigned_put_to_what_was_signed`; the stub is ours, so
  it cannot say this). `content-length` cannot be signed, so everything else must be, and each case sends the slot as given with **one thing changed**, asserts a `4xx` and that
  nothing was written, and ends with the control that the slot as given is taken: an `x-amz-*` header the signature does not cover (`x-amz-acl`, an extra `x-amz-meta-*`,
  `x-amz-storage-class`, `x-amz-server-side-encryption`, `x-amz-tagging`); `x-amz-copy-source` naming another thread's object, with the declared bytes and with none, and the key
  never ends up holding that object (the other object is untouched); a `content-type` other than the one signed (`text/html`, `image/svg+xml`, the same with a parameter, none);
  `GET`, `HEAD` and `DELETE` on the `PUT` URL, with no header and with the slot's, before and after the upload (the bytes are not given, the object stays); and the same signature
  on another hash or another thread. **Run 2026-10-14 against RustFS 1.0.1-preview.17** (the pin of `compose.yaml`): every one is refused with `403` (`AccessDenied: There were
  headers present in the request which were not signed` for the extra headers, `SignatureDoesNotMatch` for the rest), and other bytes under the signed checksum are `400 BadDigest`
  with nothing stored; `HEAD` with checksum mode says the checksum. CI runs them in `orchestrator.yml`. A server that **accepts** one of these requests fails the test; that is a
  finding for the owner, not a flaky test.
* **Presigned downloads** (A1). Always, over the stub (which answers a presigned `GET` with the signed `response-*` members as the headers of the answer, and refuses a presigned request past its
  `X-Amz-Date` + `X-Amz-Expires`; it does **not** check a signature): the four download cases of `artifact_presign_conformance!` (overrides with spaces, quotes, `;`, `&`, `=`, `+`, `?`, `#`, `%`, `*`, `'` and UTF-8 come
  back exactly; no override changes nothing; a URL opens its key and no other; a wrong request gets no URL); a download is signed for the **public** host with the bucket first in the path, the prefix and the key after it,
  `X-Amz-SignedHeaders` exactly `host`, the three members equal to what was asked and no `+` in the query, an expiry in whole seconds that is never later than the signature's own (`X-Amz-Date` + `X-Amz-Expires`), **signing sent nothing to anybody**, and the URL is shown by no `Debug`;
  a wrong download is never signed (an override with a CR LF, a lifetime of zero, of half a second or over seven days, a store with no public endpoint); a `HEAD` says the name (percent-decoded, `None` for a file with none or a
  name that is not UTF-8); a `HEAD` of an object that names another hash than its key is `Corrupt` as a `get` of it is (and one with no hash, or its own, is fine for both); every attempt of a `HEAD` is signed afresh
  (the stub fails the first one after two seconds, `Mode::FailFirst`, and the retry's `X-Amz-Date` and signature differ from the first's); a URL whose life is over is refused (a real wait of 2.5 s on a 1 s URL). With `ORCH_TEST_S3_URL`, the same suite over the server, and the module `a_real_server_holds_a_presigned_get_to_what_was_signed`
  (the stub is ours and checks no signature): the overrides come back as the headers of the answer; a `Range` is a `206` with them; a **changed**, **removed** or **added** (`response-content-encoding`, `-language`, `-expires`)
  member, another hash or thread, and the methods `HEAD`, `PUT`, `DELETE` and `POST` are each refused with `403 SignatureDoesNotMatch`, and nothing of the file is in the body; a request with no signature, with or without the
  members, is `403 AccessDenied`; a URL past its life (2 s, a real wait of 3.5 s) is `403 AccessDenied: Request has expired`, after the control that it works before. **Run 2026-10-14 against RustFS 1.0.1-preview.17** (the
  pin of `compose.yaml`): all of it passes, and the run prints that RustFS adds no `X-Content-Type-Options`, `Content-Security-Policy` or `Access-Control-Allow-Origin` and answers a preflight with `200` and no `Access-Control-*`
  header (the files host's Ingress adds them: step 6c). CI runs them in `orchestrator.yml`.
* Unit tests in `src/lib.rs`: a store needs credentials and `Debug` hides them; a key's object path is the prefix, then
  `threads/<uuid>/<hash>`, with an empty, nested or untidy prefix normalised; a `Last-Modified` that is absent or not a date is `None` ("unknown", never a default), one that is a date is read.

## See also

[`orch-ports`](../ports/README.md), [`orch-artifacts-fs`](../artifacts-fs/README.md),
[`orchestrator`](../../bin/orchestrator/README.md).
