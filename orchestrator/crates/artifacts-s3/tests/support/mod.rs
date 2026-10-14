//! A small S3 server for the tests: path-style `PUT` (and `PUT` with `x-amz-copy-source`, a copy),
//! `GET` (with `Range`), `HEAD` and `DELETE` of objects in one bucket, and `ListObjectsV2` (in pages
//! of `PAGE` keys, so a listing is followed through its continuation tokens), kept in memory. It
//! checks the access key id of the `Authorization` header or of a presigned URL's `X-Amz-Credential`
//! (not the signature), answers what S3 answers in XML, and can be told to fail, so the adapter is
//! run over real HTTP with no server to install.
//!
//! For presigned uploads it does what S3 does that the tests rely on: a presigned request must
//! carry every header its `X-Amz-SignedHeaders` lists (else `403 SignatureDoesNotMatch`), a `PUT`
//! whose body is not the `x-amz-checksum-sha256` it carries is `400 BadDigest` (unless told not to
//! enforce, like a server that does not), and a `HEAD` that asks for the checksum
//! (`x-amz-checksum-mode`) is told the one the object was put with (unless told to stay silent). A
//! presigned `GET` is answered with the `response-content-disposition`, `response-content-type` and
//! `response-cache-control` of its query as the headers of the answer, and a presigned request whose
//! `X-Amz-Date` and `X-Amz-Expires` are in the past is `403 AccessDenied`. The stub does **not**
//! check a signature, so what a signature binds (a changed override, key or method) is proven only
//! by a real S3-compatible server, the other half of the test (`ORCH_TEST_S3_URL`).
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::{Body, Bytes, to_bytes};
use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, header};
use axum::response::Response;
use sha2::{Digest as _, Sha256};

pub const BUCKET: &str = "bucket";
/// Keys a listing answers with at most, whatever `max-keys` says: small, so that a case with more
/// keys than that reads more than one page.
pub const PAGE: usize = 10;
pub const ACCESS_KEY_ID: &str = "AKIDSTUBEXAMPLE";
pub const SECRET_ACCESS_KEY: &str = "stub-secret-access-key-0123456789";

#[derive(Clone)]
pub struct Object {
    pub body: Bytes,
    /// `content-type`, every `x-amz-meta-*` header and the checksum header, as they came.
    pub headers: Vec<(HeaderName, HeaderValue)>,
    /// When it was put, as an HTTP date.
    pub modified: String,
}

/// What the stub was asked.
#[derive(Clone, Debug)]
pub struct Seen {
    pub method: Method,
    pub path: String,
    pub query: String,
    pub access_key_id: Option<String>,
    pub headers: HeaderMap,
}

/// How the stub answers.
#[derive(Clone, Copy, Debug)]
pub enum Mode {
    Up,
    /// Every request is answered with this status.
    Fail(StatusCode),
}

/// How the stub treats checksums.
#[derive(Clone, Copy, Debug)]
pub struct Checksums {
    /// A `PUT` whose body is not the `x-amz-checksum-sha256` it carries is refused.
    pub enforce: bool,
    /// A `HEAD` that asks says the checksum the object was put with.
    pub report: bool,
}

#[derive(Clone)]
pub struct Stub {
    objects: Arc<Mutex<HashMap<String, Object>>>,
    seen: Arc<Mutex<Vec<Seen>>>,
    mode: Arc<Mutex<Mode>>,
    checksums: Arc<Mutex<Checksums>>,
    pub addr: SocketAddr,
}

impl Stub {
    pub async fn start() -> Stub {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let stub = Stub {
            objects: Arc::default(),
            seen: Arc::default(),
            mode: Arc::new(Mutex::new(Mode::Up)),
            checksums: Arc::new(Mutex::new(Checksums {
                enforce: true,
                report: true,
            })),
            addr,
        };
        let app = Router::new().fallback(handle).with_state(stub.clone());
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        stub
    }

    pub fn endpoint(&self) -> String {
        format!("http://{}", self.addr)
    }

    pub fn set_mode(&self, mode: Mode) {
        *self.mode.lock().unwrap() = mode;
    }

    pub fn set_checksums(&self, checksums: Checksums) {
        *self.checksums.lock().unwrap() = checksums;
    }

    pub fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }

    pub fn object(&self, key: &str) -> Option<Object> {
        self.objects.lock().unwrap().get(key).cloned()
    }

    pub fn keys(&self) -> Vec<String> {
        let mut keys: Vec<_> = self.objects.lock().unwrap().keys().cloned().collect();
        keys.sort();
        keys
    }

    /// An object put by someone else, with these headers.
    pub fn insert(&self, key: &str, body: &[u8], headers: &[(&'static str, &str)]) {
        let headers = headers
            .iter()
            .map(|(k, v)| {
                (
                    HeaderName::from_static(k),
                    HeaderValue::from_str(v).unwrap(),
                )
            })
            .collect();
        self.objects.lock().unwrap().insert(
            key.to_owned(),
            Object {
                body: Bytes::copy_from_slice(body),
                headers,
                modified: http_date_now(),
            },
        );
    }
}

fn xml(status: StatusCode, code: &str) -> Response {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/xml")
        .body(Body::from(format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?><Error><Code>{code}</Code><Message>{code}</Message></Error>"
        )))
        .unwrap()
}

fn access_key_id(headers: &HeaderMap, query: &str) -> Option<String> {
    if let Some(auth) = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
    {
        let credential = auth.split("Credential=").nth(1)?;
        return Some(credential.split('/').next()?.to_owned());
    }
    // a presigned URL carries it in the query
    let credential = query_param(query, "X-Amz-Credential")?;
    Some(credential.split('/').next()?.to_owned())
}

fn query_param(query: &str, name: &str) -> Option<String> {
    query.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k == name).then(|| percent_decode(&v.replace('+', " ")))
    })
}

fn http_date_now() -> String {
    jiff::fmt::rfc2822::DateTimePrinter::new()
        .timestamp_to_rfc9110_string(&jiff::Timestamp::now())
        .unwrap()
}

/// What S3 refuses of a presigned request: a header its `X-Amz-SignedHeaders` lists that the
/// request does not carry.
fn missing_signed_header(headers: &HeaderMap, query: &str) -> Option<String> {
    let signed = query_param(query, "X-Amz-SignedHeaders")?;
    signed
        .split(';')
        .filter(|name| *name != "host")
        .find(|name| !headers.contains_key(*name))
        .map(str::to_owned)
}

fn base64(bytes: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

async fn handle(State(stub): State<Stub>, request: Request) -> Response {
    let (parts, body) = request.into_parts();
    let path = parts.uri.path().to_owned();
    let query = parts.uri.query().unwrap_or_default().to_owned();
    let seen = Seen {
        method: parts.method.clone(),
        path: path.clone(),
        query: query.clone(),
        access_key_id: access_key_id(&parts.headers, &query),
        headers: parts.headers.clone(),
    };
    let id = seen.access_key_id.clone();
    stub.seen.lock().unwrap().push(seen);

    let mode = *stub.mode.lock().unwrap();
    if let Mode::Fail(status) = mode {
        return xml(status, "InternalError");
    }
    if id.as_deref() != Some(ACCESS_KEY_ID) {
        return xml(StatusCode::FORBIDDEN, "InvalidAccessKeyId");
    }
    if query_param(&query, "X-Amz-Signature").is_some() {
        // a presigned request: it is good only if it carries what it signed
        if missing_signed_header(&parts.headers, &query).is_some() {
            return xml(StatusCode::FORBIDDEN, "SignatureDoesNotMatch");
        }
        let Some(expires) =
            query_param(&query, "X-Amz-Expires").and_then(|v| v.parse::<i64>().ok())
        else {
            return xml(StatusCode::FORBIDDEN, "AuthorizationQueryParametersError");
        };
        // the request is good from `X-Amz-Date` for `X-Amz-Expires` seconds
        let signed_at = query_param(&query, "X-Amz-Date").and_then(|v| {
            jiff::civil::DateTime::strptime("%Y%m%dT%H%M%SZ", &v)
                .ok()?
                .to_zoned(jiff::tz::TimeZone::UTC)
                .ok()
        });
        match signed_at {
            Some(at)
                if jiff::Timestamp::now().as_second() <= at.timestamp().as_second() + expires => {}
            _ => return xml(StatusCode::FORBIDDEN, "AccessDenied"),
        }
    }
    if parts.method == Method::GET && path.trim_end_matches('/') == format!("/{BUCKET}") {
        return list(&stub, &query);
    }
    let Some(key) = path
        .strip_prefix('/')
        .and_then(|p| p.strip_prefix(BUCKET))
        .and_then(|p| p.strip_prefix('/'))
    else {
        return xml(StatusCode::NOT_FOUND, "NoSuchBucket");
    };
    let key = key.to_owned();
    match parts.method {
        Method::PUT if parts.headers.contains_key("x-amz-copy-source") => {
            // `CopyObject`: the source is `<bucket>/<key>`, percent-encoded; the object's content
            // type and user metadata go with it (the default directive, COPY).
            let source = parts.headers["x-amz-copy-source"].to_str().unwrap();
            let source = percent_decode(source);
            let Some(source) = source
                .strip_prefix('/')
                .unwrap_or(&source)
                .strip_prefix(BUCKET)
                .and_then(|p| p.strip_prefix('/'))
                .map(str::to_owned)
            else {
                return xml(StatusCode::NOT_FOUND, "NoSuchBucket");
            };
            let Some(object) = stub.object(&source) else {
                return xml(StatusCode::NOT_FOUND, "NoSuchKey");
            };
            stub.objects.lock().unwrap().insert(key, object);
            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "application/xml")
                .body(Body::from(
                    "<?xml version=\"1.0\" encoding=\"UTF-8\"?><CopyObjectResult>\
                     <ETag>\"d41d8cd98f00b204e9800998ecf8427e\"</ETag>\
                     <LastModified>2025-01-01T00:00:00.000Z</LastModified></CopyObjectResult>",
                ))
                .unwrap()
        }
        Method::PUT => {
            let body = to_bytes(body, 64 * 1024 * 1024).await.unwrap();
            let checksums = *stub.checksums.lock().unwrap();
            if checksums.enforce
                && let Some(signed) = parts
                    .headers
                    .get("x-amz-checksum-sha256")
                    .and_then(|v| v.to_str().ok())
                && signed != base64(&Sha256::digest(&body))
            {
                return xml(StatusCode::BAD_REQUEST, "BadDigest");
            }
            let headers = parts
                .headers
                .iter()
                .filter(|(name, _)| {
                    name.as_str() == "content-type"
                        || name.as_str().starts_with("x-amz-meta-")
                        || name.as_str() == "x-amz-checksum-sha256"
                })
                // what the server stores is the checksum it computed of what it got (when the
                // request carried one), which equals the signed one when it enforced it
                .map(|(n, v)| {
                    if n.as_str() == "x-amz-checksum-sha256" {
                        let computed = base64(&Sha256::digest(&body));
                        (n.clone(), HeaderValue::from_str(&computed).unwrap())
                    } else {
                        (n.clone(), v.clone())
                    }
                })
                .collect();
            stub.objects.lock().unwrap().insert(
                key,
                Object {
                    body,
                    headers,
                    modified: http_date_now(),
                },
            );
            Response::builder()
                .status(StatusCode::OK)
                .header(header::ETAG, "\"d41d8cd98f00b204e9800998ecf8427e\"")
                .body(Body::empty())
                .unwrap()
        }
        Method::GET | Method::HEAD => {
            let Some(object) = stub.object(&key) else {
                return xml(StatusCode::NOT_FOUND, "NoSuchKey");
            };
            let asks_checksum = parts
                .headers
                .get("x-amz-checksum-mode")
                .is_some_and(|v| v == "ENABLED");
            let reports = stub.checksums.lock().unwrap().report;
            let range = parts
                .headers
                .get(header::RANGE)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.strip_prefix("bytes="))
                .and_then(|v| v.split_once('-'))
                .and_then(|(from, to)| {
                    Some((from.parse::<usize>().ok()?, to.parse::<usize>().ok()))
                });
            let (status, slice) = match range {
                Some((from, _)) if from >= object.body.len() && parts.method == Method::GET => {
                    return xml(StatusCode::RANGE_NOT_SATISFIABLE, "InvalidRange");
                }
                Some((from, to)) => {
                    let end = to.map_or(object.body.len(), |t| (t + 1).min(object.body.len()));
                    (StatusCode::PARTIAL_CONTENT, object.body.slice(from..end))
                }
                None => (StatusCode::OK, object.body.clone()),
            };
            let mut response = Response::builder()
                .status(status)
                .header(header::ETAG, "\"d41d8cd98f00b204e9800998ecf8427e\"")
                .header(header::LAST_MODIFIED, object.modified.as_str())
                .header(header::CONTENT_LENGTH, slice.len());
            if status == StatusCode::PARTIAL_CONTENT {
                response = response.header(
                    header::CONTENT_RANGE,
                    format!(
                        "bytes 0-{}/{}",
                        slice.len().saturating_sub(1),
                        object.body.len()
                    ),
                );
            }
            // the signed `response-*` members change the headers of the answer
            let overrides = [
                (header::CONTENT_TYPE, "response-content-type"),
                (header::CONTENT_DISPOSITION, "response-content-disposition"),
                (header::CACHE_CONTROL, "response-cache-control"),
            ]
            .map(|(name, param)| (name, query_param(&query, param)));
            for (name, value) in &object.headers {
                // the checksum is said only to a request that asks, and only by a server that says
                if name.as_str() == "x-amz-checksum-sha256" && !(asks_checksum && reports) {
                    continue;
                }
                if overrides.iter().any(|(n, v)| n == name && v.is_some()) {
                    continue;
                }
                response = response.header(name, value);
            }
            for (name, value) in overrides {
                if let Some(value) = value {
                    response = response.header(name, value);
                }
            }
            if asks_checksum
                && reports
                && object
                    .headers
                    .iter()
                    .any(|(n, _)| n == "x-amz-checksum-sha256")
            {
                response = response.header("x-amz-checksum-type", "FULL_OBJECT");
            }
            let body = if parts.method == Method::GET {
                Body::from(slice)
            } else {
                Body::empty()
            };
            response.body(body).unwrap()
        }
        Method::DELETE => {
            stub.objects.lock().unwrap().remove(&key);
            Response::builder()
                .status(StatusCode::NO_CONTENT)
                .body(Body::empty())
                .unwrap()
        }
        _ => xml(StatusCode::METHOD_NOT_ALLOWED, "MethodNotAllowed"),
    }
}

/// `ListObjectsV2`: the keys under `prefix`, after `continuation-token` (the last key of the page
/// before), at most [`PAGE`] of them.
fn list(stub: &Stub, query: &str) -> Response {
    let param = |name: &str| {
        query.split('&').find_map(|pair| {
            let (k, v) = pair.split_once('=')?;
            (k == name).then(|| percent_decode(&v.replace('+', " ")))
        })
    };
    let prefix = param("prefix").unwrap_or_default();
    let after = param("continuation-token").unwrap_or_default();
    let keys: Vec<String> = stub
        .keys()
        .into_iter()
        .filter(|k| k.starts_with(&prefix) && k.as_str() > after.as_str())
        .collect();
    let page: Vec<&String> = keys.iter().take(PAGE).collect();
    let truncated = keys.len() > page.len();
    let mut body = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?><ListBucketResult>");
    body.push_str(&format!(
        "<Name>{BUCKET}</Name><Prefix>{prefix}</Prefix><KeyCount>{}</KeyCount><IsTruncated>{truncated}</IsTruncated>",
        page.len()
    ));
    if truncated && let Some(last) = page.last() {
        body.push_str(&format!(
            "<NextContinuationToken>{last}</NextContinuationToken>"
        ));
    }
    for key in page {
        let size = stub.object(key).map_or(0, |o| o.body.len());
        body.push_str(&format!(
            "<Contents><Key>{key}</Key><LastModified>2025-01-01T00:00:00.000Z</LastModified>\
             <ETag>\"d41d8cd98f00b204e9800998ecf8427e\"</ETag><Size>{size}</Size></Contents>"
        ));
    }
    body.push_str("</ListBucketResult>");
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/xml")
        .body(Body::from(body))
        .unwrap()
}

/// `%XX` escapes decoded; what S3 keys of this store hold (a UUID, hex digits, a prefix) needs
/// little more.
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(byte) = u8::from_str_radix(&text[i + 1..i + 3], 16)
        {
            out.push(byte);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).unwrap()
}
