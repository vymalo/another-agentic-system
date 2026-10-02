//! Files an agent handed over (ADR 0032): the ingest, on the worker, before the commit.
//!
//! An adapter reports a file as [`AgentUpdate::File`] with its bytes. Before the core sees anything
//! the worker checks the size, sniffs the type, hashes the content and **puts it into the artifact
//! store**; only then does the core get [`AgentUpdate::FileKept`], which logs the reference. A
//! file that could not be kept reaches the core as [`AgentUpdate::FileRefused`]. The bytes never
//! reach the core, and so never the log; a reference never points at nothing (the put comes first),
//! and a file stored whose commit was lost is found again by the retry, which puts the same key.
//!
//! The limits are the configuration's (`artifacts.maxFileBytes`, `artifacts.maxPerJobBytes`) and
//! 50 files per job. A job is one delegation (one run of the agent); the count is the worker's, kept
//! for the length of its processing of the row (a worker that dies and is replaced counts again
//! from nothing, and puts the same keys, which is idempotent).

use std::collections::HashSet;
use std::sync::{Mutex, PoisonError};

use bytes::Bytes;
use orch_core::{AgentUpdate, FileRef, FileRefusal, report};
use orch_ports::{ArtifactError, ArtifactMeta, ArtifactStore, Ports};

use super::{Ctx, Dispatcher};

/// The most files a job keeps (not a configuration key: ADR 0032).
pub const MAX_FILES_PER_JOB: u32 = 50;

/// The limits the ingest enforces. The store enforces none (it is policy, not storage).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileLimits {
    /// `artifacts.maxFileBytes`: the largest file kept.
    pub max_file_bytes: u64,
    /// `artifacts.maxPerJobBytes`: the most bytes of files one job keeps.
    pub max_per_job_bytes: u64,
    /// The most files one job keeps ([`MAX_FILES_PER_JOB`]).
    pub max_files_per_job: u32,
}

impl Default for FileLimits {
    fn default() -> Self {
        FileLimits {
            max_file_bytes: 10 * 1024 * 1024,
            max_per_job_bytes: 100 * 1024 * 1024,
            max_files_per_job: MAX_FILES_PER_JOB,
        }
    }
}

/// What one delegation has kept so far.
#[derive(Debug, Default)]
pub(super) struct Budget {
    inner: Mutex<Spent>,
}

#[derive(Debug, Default)]
struct Spent {
    files: u32,
    bytes: u64,
    /// The contents already kept: the same content again is the same object, and costs nothing.
    seen: HashSet<[u8; 32]>,
}

impl Budget {
    fn lock(&self) -> std::sync::MutexGuard<'_, Spent> {
        // nothing is awaited under this lock, and a panic leaves plain counters
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Whether `size` more bytes of a content not seen yet fit under `limits`.
    fn admits(&self, limits: &FileLimits, sha256: &[u8; 32], size: u64) -> bool {
        let spent = self.lock();
        spent.seen.contains(sha256)
            || (spent.files < limits.max_files_per_job
                && spent.bytes.saturating_add(size) <= limits.max_per_job_bytes)
    }

    /// Counts a file that was kept.
    fn spend(&self, sha256: [u8; 32], size: u64) {
        let mut spent = self.lock();
        if spent.seen.insert(sha256) {
            spent.files += 1;
            spent.bytes = spent.bytes.saturating_add(size);
        }
    }
}

impl<P: Ports> Dispatcher<P> {
    /// The update the core is given for a file an adapter reported: with the file put in the store, a
    /// reference to it; or a refusal. `None` for any update that is not a file.
    pub(super) async fn ingest(&self, ctx: &Ctx, update: &AgentUpdate) -> Option<AgentUpdate> {
        let AgentUpdate::File {
            name,
            media_type,
            filename,
            bytes,
        } = update
        else {
            return None;
        };
        let refused = |reason| AgentUpdate::FileRefused {
            name: name.clone(),
            mime_type: media_type.clone(),
            reason,
        };
        let limits = self.app.file_limits();
        // The size is checked before anything is copied or hashed.
        if bytes.len() as u64 > limits.max_file_bytes {
            tracing::info!(
                thread = %ctx.thread,
                size = bytes.len(),
                max = limits.max_file_bytes,
                "a file from the agent is too large to keep"
            );
            return Some(refused(FileRefusal::TooLarge));
        }
        let mime = sniff(media_type.as_deref(), filename.as_deref(), bytes);
        let filename = filename.as_deref().and_then(clean_filename);
        let meta = ArtifactMeta::of(mime.clone(), filename.clone(), bytes);
        if !ctx.files.admits(limits, &meta.sha256, meta.size) {
            tracing::info!(thread = %ctx.thread, "the job has kept as many files as it may");
            return Some(refused(FileRefusal::JobLimit));
        }
        let key = meta.key(ctx.thread);
        match self
            .app
            .ports()
            .artifacts()
            .put(&key, Bytes::copy_from_slice(bytes), &meta)
            .await
        {
            Ok(()) => {
                ctx.files.spend(meta.sha256, meta.size);
                Some(AgentUpdate::FileKept {
                    name: name.clone(),
                    mime_type: mime,
                    file: FileRef {
                        sha256: key.sha256_hex(),
                        size: meta.size,
                        filename,
                    },
                })
            }
            Err(ArtifactError::NotConfigured) => {
                tracing::debug!(thread = %ctx.thread, "a file from the agent was not kept: no artifact store is configured");
                Some(refused(FileRefusal::NotKept))
            }
            Err(e) => {
                // The error holds no credential; the file's content is not in it.
                tracing::warn!(thread = %ctx.thread, error = %report(&e), "a file from the agent could not be put in the artifact store");
                Some(refused(FileRefusal::NotKept))
            }
        }
    }
}

/// A file name the agent gave, reduced to one name: the part after the last separator, with no
/// control or direction-changing character, at most 255 bytes. `None` when nothing is left.
pub fn clean_filename(raw: &str) -> Option<String> {
    let last = raw.rsplit(['/', '\\']).next().unwrap_or(raw);
    let mut name: String = last
        .chars()
        .filter(|c| {
            !c.is_control()
                && !matches!(*c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{200e}' | '\u{200f}')
        })
        .collect();
    name = name.trim_matches([' ', '.']).to_owned();
    while name.len() > orch_ports::MAX_NAME_BYTES {
        name.pop();
    }
    (!name.is_empty()).then_some(name)
}

/// The media type a file is kept and served as: what the agent declared when the bytes agree with
/// it, and otherwise what they are. For an image the declared type **must agree** with the magic
/// bytes, else `application/octet-stream`; a declared type that is not an image is kept as
/// declared (it is served as an attachment unless it is one of the preview types, and always with
/// `nosniff`); no declared type (or the generic one) takes the type of the bytes, or of the file
/// name's extension for a few plain types.
pub fn sniff(declared: Option<&str>, filename: Option<&str>, bytes: &[u8]) -> String {
    const OCTET: &str = "application/octet-stream";
    let declared = declared.and_then(normalise);
    let magic = image_type(bytes);
    match declared.as_deref() {
        Some(d) if d.starts_with("image/") => {
            let d = canonical_image(d);
            if magic == Some(d) {
                d.to_owned()
            } else {
                OCTET.to_owned()
            }
        }
        Some(d) if d != OCTET => d.to_owned(),
        // absent, or the generic type: the bytes, then the name
        _ => magic
            .map(str::to_owned)
            .or_else(|| (bytes.starts_with(b"%PDF-")).then(|| "application/pdf".to_owned()))
            .or_else(|| filename.and_then(by_extension).map(str::to_owned))
            .unwrap_or_else(|| OCTET.to_owned()),
    }
}

/// `type/subtype` in lower case without parameters, when it is one.
fn normalise(declared: &str) -> Option<String> {
    let essence = declared.split(';').next()?.trim().to_ascii_lowercase();
    let (kind, subtype) = essence.split_once('/')?;
    let token = |s: &str| {
        !s.is_empty()
            && s.len() <= 127
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"!#$&-^_.+".contains(&b))
    };
    (token(kind) && token(subtype)).then_some(essence)
}

/// The spellings of one image type that are in use.
fn canonical_image(declared: &str) -> &str {
    match declared {
        "image/jpg" | "image/pjpeg" => "image/jpeg",
        "image/svg" => "image/svg+xml",
        other => other,
    }
}

/// The image type the bytes begin as, for the types a browser draws and a few more.
fn image_type(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else if bytes.starts_with(b"BM") && bytes.len() > 14 {
        Some("image/bmp")
    } else if bytes.starts_with(&[0, 0, 1, 0]) {
        Some("image/x-icon")
    } else if bytes.starts_with(b"II*\0") || bytes.starts_with(b"MM\0*") {
        Some("image/tiff")
    } else if bytes.len() >= 12 && &bytes[4..8] == b"ftyp" && &bytes[8..12] == b"avif" {
        Some("image/avif")
    } else if looks_like_svg(bytes) {
        Some("image/svg+xml")
    } else {
        None
    }
}

/// An SVG document: after a byte order mark, blanks, an XML declaration, comments and a DOCTYPE,
/// an `<svg` element, within the first 4 KiB.
fn looks_like_svg(bytes: &[u8]) -> bool {
    let head = &bytes[..bytes.len().min(4096)];
    let head = head.strip_prefix(b"\xef\xbb\xbf").unwrap_or(head);
    let Ok(text) =
        std::str::from_utf8(head).or_else(|e| std::str::from_utf8(&head[..e.valid_up_to()]))
    else {
        return false;
    };
    let mut rest = text.trim_start();
    loop {
        if let Some(after) = rest.strip_prefix("<?") {
            let Some((_, tail)) = after.split_once("?>") else {
                return false;
            };
            rest = tail.trim_start();
        } else if let Some(after) = rest.strip_prefix("<!--") {
            let Some((_, tail)) = after.split_once("-->") else {
                return false;
            };
            rest = tail.trim_start();
        } else if rest.len() >= 9 && rest[..9].eq_ignore_ascii_case("<!doctype") {
            let Some((_, tail)) = rest.split_once('>') else {
                return false;
            };
            rest = tail.trim_start();
        } else {
            break;
        }
    }
    rest.starts_with("<svg") && rest[4..].starts_with([' ', '\t', '\r', '\n', '>', '/'])
}

/// The type of a few plain files by the extension of their name.
fn by_extension(filename: &str) -> Option<&'static str> {
    let extension = filename.rsplit_once('.')?.1.to_ascii_lowercase();
    match extension.as_str() {
        "txt" | "log" => Some("text/plain"),
        "md" => Some("text/markdown"),
        "csv" => Some("text/csv"),
        "json" => Some("application/json"),
        "pdf" => Some("application/pdf"),
        "zip" => Some("application/zip"),
        _ => None,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";

    #[test]
    fn an_image_must_agree_with_its_bytes() {
        assert_eq!(sniff(Some("image/png"), None, PNG), "image/png");
        assert_eq!(sniff(Some("IMAGE/PNG; charset=x"), None, PNG), "image/png");
        assert_eq!(
            sniff(Some("image/png"), None, b"<html><script>alert(1)</script>"),
            "application/octet-stream"
        );
        assert_eq!(
            sniff(Some("image/jpeg"), None, PNG),
            "application/octet-stream",
            "a png declared as a jpeg"
        );
        assert_eq!(
            sniff(Some("image/jpg"), None, &[0xff, 0xd8, 0xff, 0xe0]),
            "image/jpeg"
        );
        assert_eq!(
            sniff(Some("image/png"), Some("a.png"), b""),
            "application/octet-stream",
            "an empty file is not an image"
        );
        // a type the sniff has no bytes for cannot be agreed with
        assert_eq!(
            sniff(Some("image/x-unheard-of"), None, PNG),
            "application/octet-stream"
        );
    }

    #[test]
    fn svg_is_known_by_its_content() {
        let svg = br#"<?xml version="1.0"?><!-- c --><svg xmlns="http://www.w3.org/2000/svg"/>"#;
        assert_eq!(sniff(Some("image/svg+xml"), None, svg), "image/svg+xml");
        assert_eq!(sniff(Some("image/svg"), None, svg), "image/svg+xml");
        assert_eq!(sniff(None, None, svg), "image/svg+xml");
        assert_eq!(
            sniff(Some("image/svg+xml"), None, b"<svgx/>"),
            "application/octet-stream"
        );
        assert_eq!(
            sniff(Some("image/svg+xml"), None, b"<html><svg/></html>"),
            "application/octet-stream"
        );
        assert_eq!(
            sniff(
                Some("image/svg+xml"),
                None,
                b"\xef\xbb\xbf  <svg width='1'/>"
            ),
            "image/svg+xml"
        );
    }

    #[test]
    fn other_types_are_kept_as_declared_and_missing_ones_are_found() {
        assert_eq!(
            sniff(Some("text/plain; charset=utf-8"), None, b"hi"),
            "text/plain"
        );
        assert_eq!(
            sniff(Some("application/pdf"), None, b"x"),
            "application/pdf"
        );
        assert_eq!(sniff(Some("text/html"), None, b"<b>"), "text/html");
        assert_eq!(sniff(None, None, PNG), "image/png");
        assert_eq!(
            sniff(Some("application/octet-stream"), None, PNG),
            "image/png"
        );
        assert_eq!(sniff(None, None, b"%PDF-1.7"), "application/pdf");
        assert_eq!(sniff(None, Some("notes.TXT"), b"hi"), "text/plain");
        assert_eq!(sniff(None, Some("data.json"), b"{}"), "application/json");
        assert_eq!(
            sniff(None, Some("blob"), b"\0\0"),
            "application/octet-stream"
        );
        assert_eq!(
            sniff(Some("not a type"), None, b"x"),
            "application/octet-stream"
        );
        assert_eq!(sniff(Some("a/b/c"), None, b"x"), "application/octet-stream");
        assert_eq!(
            sniff(Some("text/pl ain"), None, b"x"),
            "application/octet-stream"
        );
    }

    #[test]
    fn every_magic_is_found() {
        let cases: [(&[u8], &str); 7] = [
            (b"GIF89a....", "image/gif"),
            (b"GIF87a....", "image/gif"),
            (b"RIFF\x10\0\0\0WEBPVP8 ", "image/webp"),
            (b"BM\0\0\0\0\0\0\0\0\0\0\0\0\0\0", "image/bmp"),
            (b"II*\0....", "image/tiff"),
            (b"\0\0\0\x1cftypavif....", "image/avif"),
            (&[0, 0, 1, 0, 1, 0], "image/x-icon"),
        ];
        for (bytes, expected) in cases {
            assert_eq!(sniff(Some(expected), None, bytes), expected);
        }
        // RIFF that is not a webp
        assert_eq!(
            sniff(Some("image/webp"), None, b"RIFF\x10\0\0\0WAVEfmt "),
            "application/octet-stream"
        );
    }

    #[test]
    fn a_file_name_is_one_name() {
        let clean = |s: &str| clean_filename(s);
        assert_eq!(clean("chart.png").as_deref(), Some("chart.png"));
        assert_eq!(clean("../../etc/passwd").as_deref(), Some("passwd"));
        assert_eq!(
            clean(r"C:\Users\me\report.pdf").as_deref(),
            Some("report.pdf")
        );
        assert_eq!(clean("a\nb\0c.txt").as_deref(), Some("abc.txt"));
        assert_eq!(clean("evil\u{202e}gnp.exe").as_deref(), Some("evilgnp.exe"));
        assert_eq!(clean("..").as_deref(), None);
        assert_eq!(clean("  . ").as_deref(), None);
        assert_eq!(clean("dir/").as_deref(), None);
        assert_eq!(clean("").as_deref(), None);
        assert_eq!(clean("é.txt").as_deref(), Some("é.txt"));
        let long = "x".repeat(400);
        assert_eq!(clean(&long).unwrap().len(), 255);
        let wide = "é".repeat(200);
        assert!(clean(&wide).unwrap().len() <= 255);
    }

    #[test]
    fn the_budget_counts_files_and_bytes_and_each_content_once() {
        let limits = FileLimits {
            max_file_bytes: 100,
            max_per_job_bytes: 10,
            max_files_per_job: 2,
        };
        let budget = Budget::default();
        let (a, b, c) = ([1; 32], [2; 32], [3; 32]);
        assert!(budget.admits(&limits, &a, 6));
        assert!(
            !budget.admits(&limits, &b, 11),
            "over the bytes of a job on its own"
        );
        budget.spend(a, 6);
        assert!(!budget.admits(&limits, &b, 5), "6 + 5 is over 10");
        assert!(
            budget.admits(&limits, &a, 6),
            "the same content again is free"
        );
        assert!(budget.admits(&limits, &b, 4));
        budget.spend(b, 4);
        budget.spend(a, 6);
        assert!(!budget.admits(&limits, &c, 0), "two files are the most");
        assert!(budget.admits(&limits, &b, 4));
    }
}
