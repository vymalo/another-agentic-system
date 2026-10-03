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
| `S3Config::new(bucket)` | `.with_region(r)` (default `us-east-1`, what compatible servers expect), `.with_endpoint(url)` (a server other than AWS: the bucket is then in the path, `<endpoint>/<bucket>/<key>`; `http://` is allowed), `.with_prefix(p)` (keys go under `p/`; one bucket for several deployments), `.with_credentials(access_key_id, secret_access_key)` (`SecretString`s; **required**), `.with_timeout(d)` (one request, default 60 s). `Debug` shows `<redacted>` for the credentials |
| `BuildError::Config(text)` | no credentials, or a configuration the library refuses; the text never holds a credential |
| `impl ArtifactStore for S3Artifacts` | `put`, `get`, `delete`, `copy` and `delete_prefix` as the port says |

## How a file is kept

* One object per file, `[<prefix>/]threads/<thread uuid>/<sha256>`. The **media type is the object's `Content-Type`**; the hash
  is user metadata `x-amz-meta-sha256`, and the file name `x-amz-meta-filename`, **percent-encoded** because a metadata
  value is ASCII (a file with no name has no such header). The size is the object's own. No sidecar object: a put is one
  `PUT`, atomic on the server's side, so a file is either whole with its meta or not there.
* **Idempotent by content**: a put again overwrites the object with the same bytes (and the meta of the second call).
* `get` reads the object as a stream. An object whose `x-amz-meta-sha256` names another hash, that has no content type or
  whose file name is not UTF-8 is `ArtifactError::Corrupt` (never served); an object somebody else wrote with a content type
  and no user metadata reads fine, with no file name.
* `delete` is one `DELETE` (the bulk call is turned off: not every compatible server has it). A key that is not there is fine.
* `delete_prefix(thread)` (the erasure of a deleted thread's files, [ADR 0043](../../../docs/decisions/0043-deleting-a-thread-erases-it.md)) **lists** `[<prefix>/]threads/<thread uuid>/` (`ListObjectsV2`, followed through its continuation tokens; the listing is by whole path segments, so no other thread's and no other prefix's objects) and deletes each object with its own `DELETE`, twenty at a time (the bulk `DeleteObjects` stays off, as for `delete`). It returns how many objects it deleted; a thread with none is `Ok(0)`. The library reports a refused *listing* as a failed request, so a refusal there is `Unavailable` and not `Unauthenticated`; the purge that retries it does not depend on the class. Verified against `tests/support` only (2026-10-03).
* `copy` is one **server-side `CopyObject`** (`x-amz-copy-source`, no body): the bytes stay in the bucket and the object's content type and user metadata go with it. The key is the content's hash, so there is nothing to re-hash. A missing source is `NotFound`; a copy of a key onto itself (S3 refuses that) is a `HEAD`; two keys of different hashes are `Invalid` and nothing is sent. Verified against `tests/support` only (2026-10-03); a real S3-compatible server runs it through the testkit when `ORCH_TEST_S3_URL` is set, which has not been done for this change.
* **Credentials** are the two secrets of the configuration, used as given (static, SigV4). This store does not read `AWS_*`
  variables, the instance profile or a web-identity token; a deployment that needs those needs a change to this crate (not planned: the
  configuration gives both keys, by reference). They are in no error and no `Debug`.
* **Errors.** A refusal (HTTP 401 or 403) is `Unauthenticated`, permanent. Everything else the server or the network does is
  `Unavailable`, transient, with the library's error as its source (it holds the URL, so it never reaches the chat log).
  `object_store` retries a failed request twice within 30 seconds, then the caller sees the error and decides.

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
  with MinIO:

  ```sh
  docker run -d -p 9000:9000 -e MINIO_ROOT_USER=minioadmin -e MINIO_ROOT_PASSWORD=minioadmin minio/minio server /data
  mc alias set local http://127.0.0.1:9000 minioadmin minioadmin && mc mb local/orch-test
  ORCH_TEST_S3_URL=http://127.0.0.1:9000 cargo test -p orch-artifacts-s3
  ```

  The author's run (2026-10-02) was against moto 5.2.3's S3 server (`moto_server`), all twelve passing; the CI job does not
  start a server yet, so CI runs the stub only.
* Unit tests in `src/lib.rs`: a store needs credentials and `Debug` hides them; a key's object path is the prefix, then
  `threads/<uuid>/<hash>`, with an empty, nested or untidy prefix normalised.

## See also

[`orch-ports`](../ports/README.md), [`orch-artifacts-fs`](../artifacts-fs/README.md),
[`orchestrator`](../../bin/orchestrator/README.md).
