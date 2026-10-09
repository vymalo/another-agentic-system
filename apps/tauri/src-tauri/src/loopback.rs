//! Signing in through the system browser and a loopback listener (RFC 8252 sections 7.3 and 8.3; ADR 0047, decision 3).
//!
//! The page makes the PKCE verifier, the `state` and the authorization URL, as it does in a browser
//! (`web/src/lib/auth/sign-in.ts`), and asks this process for two things only: a listener on `127.0.0.1` and a free port
//! (the redirect URI is `http://127.0.0.1:<port>/callback`, which Keycloak matches with any port for the registered
//! `http://127.0.0.1/callback`), and to open the URL in the person's browser and hand back the address the issuer sent the
//! browser back to. The page checks the `state` and exchanges the code with its own DPoP proof; nothing here sees a token.
//! The listener answers one `GET /callback`, refuses anything else, and stops after five minutes.

use std::time::Duration;

use tauri::{AppHandle, Manager, State};
use tauri_plugin_opener::OpenerExt;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::Mutex;
use url::Url;

/// How long the person has to sign in, in the browser, before the listener stops.
const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(300);

/// The longest request line and headers read from the browser.
const MAX_REQUEST: usize = 16 * 1024;

/// What the browser's tab shows once it has been sent back.
const DONE_PAGE: &str = "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><title>Signed in</title></head>\
<body style=\"font-family:system-ui,sans-serif;margin:4rem auto;max-width:28rem;text-align:center\">\
<h1 style=\"font-size:1.25rem\">You can go back to the app</h1>\
<p>This tab can be closed.</p></body></html>";

/// The listener of the sign-in under way, between [`loopback_listen`] and [`loopback_sign_in`].
#[derive(Default)]
pub(crate) struct Loopback {
    listener: Mutex<Option<TcpListener>>,
}

/// Binds `127.0.0.1` on a port the system picks and returns it. A sign-in that was under way is dropped.
#[tauri::command]
pub(crate) async fn loopback_listen(state: State<'_, Loopback>) -> Result<u16, String> {
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .await
        .map_err(|e| format!("could not listen on 127.0.0.1: {e}"))?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("could not read the listener's port: {e}"))?
        .port();
    *state.listener.lock().await = Some(listener);
    Ok(port)
}

/// An address the person's browser may be sent to: the issuer over https (or http on this machine, a local issuer).
fn openable(url: &str) -> Result<Url, String> {
    let parsed = Url::parse(url).map_err(|_| "not a URL".to_owned())?;
    let local = matches!(parsed.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"));
    if parsed.scheme() == "https" || (parsed.scheme() == "http" && local) {
        Ok(parsed)
    } else {
        Err("only an https address (or http on this machine) is opened".to_owned())
    }
}

/// Opens `url` (the authorization request) in the person's browser and waits for the issuer to send the browser back to
/// the listener: returns that address, `http://127.0.0.1:<port>/callback?code=…&state=…`, and brings the window back.
#[tauri::command]
pub(crate) async fn loopback_sign_in(
    app: AppHandle,
    state: State<'_, Loopback>,
    url: String,
) -> Result<String, String> {
    let target = openable(&url)?;
    let listener = state
        .listener
        .lock()
        .await
        .take()
        .ok_or_else(|| "no listener: ask for one first".to_owned())?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    app.opener()
        .open_url(target.as_str(), None::<&str>)
        .map_err(|e| format!("could not open the browser: {e}"))?;
    let back = tokio::time::timeout(SIGN_IN_TIMEOUT, accept_callback(&listener, port))
        .await
        .map_err(|_| "the sign-in was not finished in time".to_owned())??;
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
    Ok(back)
}

/// Answers requests until one is `GET /callback?…`, which it answers with [`DONE_PAGE`] and returns as a full address.
async fn accept_callback(listener: &TcpListener, port: u16) -> Result<String, String> {
    loop {
        let (mut stream, _) = listener.accept().await.map_err(|e| e.to_string())?;
        let mut buf = vec![0_u8; MAX_REQUEST];
        let mut read = 0;
        while read < buf.len() {
            let n = stream
                .read(&mut buf[read..])
                .await
                .map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            read += n;
            if buf[..read].windows(4).any(|w| w == b"\r\n\r\n") {
                break;
            }
        }
        let head = String::from_utf8_lossy(&buf[..read]);
        let target = callback_target(&head);
        let (status, body) = match target {
            Some(_) => ("200 OK", DONE_PAGE),
            None => ("404 Not Found", "not found"),
        };
        let answer = format!(
            "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = stream.write_all(answer.as_bytes()).await;
        let _ = stream.shutdown().await;
        if let Some(path_and_query) = target {
            return Ok(format!("http://127.0.0.1:{port}{path_and_query}"));
        }
    }
}

/// The path and query of a `GET /callback?…` request line, else `None`.
fn callback_target(head: &str) -> Option<&str> {
    let line = head.lines().next()?;
    let mut parts = line.split(' ');
    let (method, target) = (parts.next()?, parts.next()?);
    (method == "GET" && (target == "/callback" || target.starts_with("/callback?")))
        .then_some(target)
}

/// Opens `url` in the person's browser (the issuer's end-session page when signing out: never inside the app).
#[tauri::command]
pub(crate) async fn open_in_browser(app: AppHandle, url: String) -> Result<(), String> {
    let target = openable(&url)?;
    app.opener()
        .open_url(target.as_str(), None::<&str>)
        .map_err(|e| format!("could not open the browser: {e}"))
}

#[cfg(test)]
mod tests {
    use super::{callback_target, openable};

    #[test]
    fn only_a_get_of_the_callback_is_the_callback() {
        assert_eq!(
            callback_target("GET /callback?code=a&state=b HTTP/1.1\r\nHost: x\r\n\r\n"),
            Some("/callback?code=a&state=b")
        );
        assert_eq!(
            callback_target("GET /callback HTTP/1.1\r\n\r\n"),
            Some("/callback")
        );
        for head in [
            "GET /favicon.ico HTTP/1.1\r\n\r\n",
            "POST /callback?code=a HTTP/1.1\r\n\r\n",
            "GET /callbackx?code=a HTTP/1.1\r\n\r\n",
            "",
        ] {
            assert_eq!(callback_target(head), None, "{head:?}");
        }
    }

    #[test]
    fn only_the_issuer_over_https_or_a_local_one_is_opened() {
        assert!(
            openable("https://auth.example.com/realms/x/protocol/openid-connect/auth?a=b").is_ok()
        );
        assert!(openable("http://127.0.0.1:8180/realms/x/auth").is_ok());
        for bad in [
            "http://auth.example.com/",
            "file:///etc/passwd",
            "javascript:alert(1)",
            "tauri://localhost/",
            "not a url",
        ] {
            assert!(openable(bad).is_err(), "{bad}");
        }
    }
}
