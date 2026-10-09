//! The static export has one page per kind of address, `/threads/_` and `/s/_`, which the web image's server answers for any
//! `/threads/<id>` and `/s/<token>` (`web/Caddyfile`, `web/src/lib/static-routes.ts`). Tauri falls back to `index.html` for an
//! asset it does not have, so the app maps the same addresses to the same files itself, around the embedded assets.
//!
//! Only what Tauri asks after the bare path is mapped: `<path>.html` (so that the answer is an `.html` asset and gets Tauri's
//! content security policy with the page's hashes), `<path>.txt` and the per-segment `<path>/__next.*.txt` (the data Next
//! fetches when the app moves between threads).

use std::borrow::Cow;

use tauri::utils::assets::{AssetKey, AssetsIter, CspHash};
use tauri::{App, Assets, Runtime};

/// The file of the export that answers `key`, when `key` is an address of a thread or a share link.
pub(crate) fn shell_of(key: &str) -> Option<String> {
    for (prefix, shell) in [("/threads/", "/threads/_"), ("/s/", "/s/_")] {
        let Some(rest) = key.strip_prefix(prefix) else {
            continue;
        };
        let (segment, tail) = match rest.split_once('/') {
            Some((segment, file)) => (segment, Some(file)),
            None => (rest, None),
        };
        let id_ok = |id: &str| !id.is_empty() && id != "_" && !id.contains('.');
        match tail {
            // `/threads/<id>.html`, `/threads/<id>.txt`
            None => {
                for ext in [".html", ".txt"] {
                    if let Some(id) = segment.strip_suffix(ext)
                        && id_ok(id)
                    {
                        return Some(format!("{shell}{ext}"));
                    }
                }
            }
            // `/threads/<id>/__next.….txt`
            Some(file)
                if id_ok(segment)
                    && !file.contains('/')
                    && file.ends_with(".txt")
                    && !file.contains("..") =>
            {
                return Some(format!("{shell}/{file}"));
            }
            Some(_) => {}
        }
    }
    None
}

fn mapped(key: &AssetKey) -> AssetKey {
    shell_of(key.as_ref()).map_or_else(|| AssetKey::from(key.as_ref()), AssetKey::from)
}

/// The embedded assets, with the addresses of threads and share links mapped to their shells.
pub(crate) struct Shells<R: Runtime> {
    inner: Box<dyn Assets<R>>,
}

impl<R: Runtime> Shells<R> {
    pub(crate) fn new(inner: Box<dyn Assets<R>>) -> Self {
        Shells { inner }
    }
}

impl<R: Runtime> Assets<R> for Shells<R> {
    fn setup(&self, app: &App<R>) {
        self.inner.setup(app);
    }

    fn get(&self, key: &AssetKey) -> Option<Cow<'_, [u8]>> {
        self.inner.get(key).or_else(|| self.inner.get(&mapped(key)))
    }

    fn iter(&self) -> Box<AssetsIter<'_>> {
        self.inner.iter()
    }

    fn csp_hashes(&self, html_path: &AssetKey) -> Box<dyn Iterator<Item = CspHash<'_>> + '_> {
        match shell_of(html_path.as_ref()) {
            Some(shell) => self.inner.csp_hashes(&AssetKey::from(shell)),
            None => self.inner.csp_hashes(html_path),
        }
    }
}

/// No assets: what stands in for a moment while the embedded ones are taken out to be wrapped.
pub(crate) struct Empty;

impl<R: Runtime> Assets<R> for Empty {
    fn get(&self, _key: &AssetKey) -> Option<Cow<'_, [u8]>> {
        None
    }

    fn iter(&self) -> Box<AssetsIter<'_>> {
        Box::new(std::iter::empty())
    }

    fn csp_hashes(&self, _html_path: &AssetKey) -> Box<dyn Iterator<Item = CspHash<'_>> + '_> {
        Box::new(std::iter::empty())
    }
}

#[cfg(test)]
mod tests {
    use super::shell_of;

    #[test]
    fn a_thread_and_a_share_link_are_their_shells() {
        let id = "/threads/0199c0de-0000-7000-8000-000000000001";
        assert_eq!(
            shell_of(&format!("{id}.html")).as_deref(),
            Some("/threads/_.html")
        );
        assert_eq!(
            shell_of(&format!("{id}.txt")).as_deref(),
            Some("/threads/_.txt")
        );
        assert_eq!(
            shell_of(&format!("{id}/__next.threads.$d$id.__PAGE__.txt")).as_deref(),
            Some("/threads/_/__next.threads.$d$id.__PAGE__.txt")
        );
        assert_eq!(shell_of("/s/abc_DEF-1.html").as_deref(), Some("/s/_.html"));
        assert_eq!(shell_of("/s/abc_DEF-1.txt").as_deref(), Some("/s/_.txt"));
    }

    #[test]
    fn every_other_address_is_itself() {
        for key in [
            "/index.html",
            "/threads/_.html",
            "/threads/.html",
            "/threads/a/b/c.txt",
            "/threads/a/../config.json",
            "/threads/a/x.js",
            "/s/",
            "/auth/callback.html",
            "/_next/static/chunks/a.js",
        ] {
            assert_eq!(shell_of(key), None, "{key}");
        }
    }
}
