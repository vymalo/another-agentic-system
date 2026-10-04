//! A small S3 server for the tests: path-style `PUT` (and `PUT` with `x-amz-copy-source`, a copy),
//! `GET`, `HEAD` and `DELETE` of objects in one bucket, and `ListObjectsV2` (in pages of `PAGE`
//! keys, so a listing is followed through its continuation tokens), kept in memory. It checks the access key id of the `Authorization` header (not the
//! signature), answers what S3 answers in XML, and can be told to fail, so the adapter is run over
//! real HTTP with no server to install. A real S3-compatible server is the other half of the test
//! (`ORCH_TEST_S3_URL`).
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::{Body, Bytes, to_bytes};
use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, header};
use axum::response::Response;

pub const BUCKET: &str = "bucket";
/// Keys a listing answers with at most, whatever `max-keys` says: small, so that a case with more
/// keys than that reads more than one page.
pub const PAGE: usize = 10;
pub const ACCESS_KEY_ID: &str = "AKIDSTUBEXAMPLE";
pub const SECRET_ACCESS_KEY: &str = "stub-secret-access-key-0123456789";

#[derive(Clone)]
pub struct Object {
    pub body: Bytes,
    /// `content-type` and every `x-amz-meta-*` header, as they came.
    pub headers: Vec<(HeaderName, HeaderValue)>,
}

/// What the stub was asked.
#[derive(Clone, Debug)]
pub struct Seen {
    pub method: Method,
    pub path: String,
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

#[derive(Clone)]
pub struct Stub {
    objects: Arc<Mutex<HashMap<String, Object>>>,
    seen: Arc<Mutex<Vec<Seen>>>,
    mode: Arc<Mutex<Mode>>,
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

fn access_key_id(headers: &HeaderMap) -> Option<String> {
    let auth = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let credential = auth.split("Credential=").nth(1)?;
    Some(credential.split('/').next()?.to_owned())
}

async fn handle(State(stub): State<Stub>, request: Request) -> Response {
    let (parts, body) = request.into_parts();
    let path = parts.uri.path().to_owned();
    let seen = Seen {
        method: parts.method.clone(),
        path: path.clone(),
        access_key_id: access_key_id(&parts.headers),
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
    if parts.method == Method::GET && path.trim_end_matches('/') == format!("/{BUCKET}") {
        return list(&stub, parts.uri.query().unwrap_or_default());
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
            let headers = parts
                .headers
                .iter()
                .filter(|(name, _)| {
                    name.as_str() == "content-type" || name.as_str().starts_with("x-amz-meta-")
                })
                .map(|(n, v)| (n.clone(), v.clone()))
                .collect();
            stub.objects
                .lock()
                .unwrap()
                .insert(key, Object { body, headers });
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
            let mut response = Response::builder()
                .status(StatusCode::OK)
                .header(header::ETAG, "\"d41d8cd98f00b204e9800998ecf8427e\"")
                .header(header::LAST_MODIFIED, "Wed, 01 Jan 2025 00:00:00 GMT")
                .header(header::CONTENT_LENGTH, object.body.len());
            for (name, value) in &object.headers {
                response = response.header(name, value);
            }
            let body = if parts.method == Method::GET {
                Body::from(object.body)
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
