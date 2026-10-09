//! Signing in through the system browser and a loopback listener (RFC 8252 sections 7.3 and 8.3; ADR 0047, decision 3).
//!
//! The page makes the PKCE verifier, the `state` and the authorization URL, as it does in a browser
//! (`web/src/lib/auth/sign-in.ts`), and asks this process for two things only: a listener on `127.0.0.1` and a free port
//! (the redirect URI is `http://127.0.0.1:<port>/callback`, which Keycloak matches with any port for the registered
//! `http://127.0.0.1/callback`), and to open the URL in the person's browser and hand back the address the issuer sent the
//! browser back to. The page checks the `state` and exchanges the code with its own DPoP proof; nothing here sees a token.
//! The listener answers one `GET /callback`, refuses anything else, and stops after five minutes or when the page cancels
//! ([`loopback_cancel`]). Only the issuer's origin, fixed when the app is built (`AGENTIC_ISSUER_ORIGIN`, `build.rs`), is ever
//! opened in the browser.

use std::task::Poll;
use std::time::Duration;

use tauri::{AppHandle, Manager, State};
use tauri_plugin_opener::OpenerExt;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::{Mutex, Notify};
use url::Url;

/// How long the person has to sign in, in the browser, before the listener stops.
const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(300);

/// How long one connection to the listener may take to send its request: a connection that sends nothing does not hold the
/// callback up.
const READ_TIMEOUT: Duration = Duration::from_secs(5);

/// The issuer's origin, the only one opened in the browser: set when the app is built (`build.rs`, `deployment.json`).
const ISSUER_ORIGIN: &str = env!("AGENTIC_ISSUER_ORIGIN");

/// The longest request line and headers read from the browser.
const MAX_REQUEST: usize = 16 * 1024;

/// What the browser's tab shows once it has been sent back.
const DONE_PAGE: &str = "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><title>Signed in</title></head>\
<body style=\"font-family:system-ui,sans-serif;margin:4rem auto;max-width:28rem;text-align:center\">\
<h1 style=\"font-size:1.25rem\">You can go back to the app</h1>\
<p>This tab can be closed.</p></body></html>";

/// The listener of the sign-in under way, between [`loopback_listen`] and [`loopback_sign_in`], and the signal that cancels it.
#[derive(Default)]
pub(crate) struct Loopback {
    listener: Mutex<Option<TcpListener>>,
    cancel: Notify,
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

/// An address the person's browser may be sent to: one of the issuer's, `issuer` being its origin (https, or http on this
/// machine for a local issuer).
fn openable(url: &str, issuer: &str) -> Result<Url, String> {
    let parsed = Url::parse(url).map_err(|_| "not a URL".to_owned())?;
    let local = matches!(parsed.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"));
    let scheme_ok = parsed.scheme() == "https" || (parsed.scheme() == "http" && local);
    if scheme_ok && parsed.origin().ascii_serialization() == issuer {
        Ok(parsed)
    } else {
        Err(format!(
            "only an address of the issuer, {issuer}, is opened"
        ))
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
    let target = openable(&url, ISSUER_ORIGIN)?;
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
    let back = first(
        async {
            tokio::time::timeout(SIGN_IN_TIMEOUT, accept_callback(&listener, port))
                .await
                .map_err(|_| "the sign-in was not finished in time".to_owned())?
        },
        async {
            state.cancel.notified().await;
            Err("cancelled".to_owned())
        },
    )
    .await?;
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
    Ok(back)
}

/// The output of whichever of `a` and `b` is ready first (`a` when both are); the other is dropped.
async fn first<T>(a: impl Future<Output = T>, b: impl Future<Output = T>) -> T {
    let mut a = std::pin::pin!(a);
    let mut b = std::pin::pin!(b);
    std::future::poll_fn(|cx| match a.as_mut().poll(cx) {
        Poll::Ready(v) => Poll::Ready(v),
        Poll::Pending => b.as_mut().poll(cx),
    })
    .await
}

/// Stops the sign-in under way, if any: its [`loopback_sign_in`] returns `cancelled` and the listener is closed. The page
/// calls it when the person gives up (the browser tab abandoned), so that the Sign in button is not held for five minutes.
#[tauri::command]
pub(crate) async fn loopback_cancel(state: State<'_, Loopback>) -> Result<(), String> {
    state.listener.lock().await.take();
    state.cancel.notify_waiters();
    Ok(())
}

/// The request line and headers of one connection, read for at most [`READ_TIMEOUT`]; `None` when it sent nothing usable.
async fn read_head(stream: &mut tokio::net::TcpStream) -> Option<String> {
    let mut buf = vec![0_u8; MAX_REQUEST];
    let mut read = 0;
    let reading = async {
        while read < buf.len() {
            let n = stream.read(&mut buf[read..]).await.ok()?;
            if n == 0 {
                break;
            }
            read += n;
            if buf[..read].windows(4).any(|w| w == b"\r\n\r\n") {
                break;
            }
        }
        Some(())
    };
    tokio::time::timeout(READ_TIMEOUT, reading).await.ok()??;
    Some(String::from_utf8_lossy(&buf[..read]).into_owned())
}

/// Answers requests until one is `GET /callback?…`, which it answers with [`DONE_PAGE`] and returns as a full address. A
/// connection that fails, is slow or asks for anything else is answered (or dropped) and the listener goes on.
async fn accept_callback(listener: &TcpListener, port: u16) -> Result<String, String> {
    loop {
        let Ok((mut stream, _)) = listener.accept().await else {
            // a connection that failed before it was accepted; brief pause in case the system is out of descriptors
            tokio::time::sleep(Duration::from_millis(50)).await;
            continue;
        };
        let Some(head) = read_head(&mut stream).await else {
            continue;
        };
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
    let target = openable(&url, ISSUER_ORIGIN)?;
    app.opener()
        .open_url(target.as_str(), None::<&str>)
        .map_err(|e| format!("could not open the browser: {e}"))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
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
    fn only_an_address_of_the_issuer_is_opened() {
        let issuer = "https://auth.example.com";
        assert!(
            openable(
                "https://auth.example.com/realms/x/protocol/openid-connect/auth?a=b",
                issuer
            )
            .is_ok()
        );
        assert!(openable("https://auth.example.com/realms/x/logout", issuer).is_ok());
        for bad in [
            "https://evil.example/realms/x/auth",
            "https://auth.example.com.evil.example/",
            "https://auth.example.com:8443/",
            "http://auth.example.com/",
            "file:///etc/passwd",
            "javascript:alert(1)",
            "tauri://localhost/",
            "not a url",
        ] {
            assert!(openable(bad, issuer).is_err(), "{bad}");
        }
        // a local issuer (the web's mock) over http on this machine, and only that one
        let local = "http://127.0.0.1:4010";
        assert!(openable("http://127.0.0.1:4010/oidc/auth?a=b", local).is_ok());
        assert!(openable("http://127.0.0.1:4011/oidc/auth", local).is_err());
        assert!(openable("http://example.com/oidc/auth", "http://example.com").is_err());
    }

    #[test]
    fn the_issuer_of_the_build_is_an_origin() {
        let parsed = url::Url::parse(super::ISSUER_ORIGIN).unwrap();
        assert_eq!(parsed.origin().ascii_serialization(), super::ISSUER_ORIGIN);
    }

    fn block_on<F: Future>(f: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(f)
    }

    #[test]
    fn a_silent_or_broken_connection_does_not_hold_up_the_callback() {
        use tokio::io::AsyncWriteExt as _;
        block_on(async {
            let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
                .await
                .unwrap();
            let port = listener.local_addr().unwrap().port();
            let waiting =
                tokio::spawn(async move { super::accept_callback(&listener, port).await });
            // one connection that says nothing, one that hangs up at once, one that asks for something else
            let _silent = tokio::net::TcpStream::connect(("127.0.0.1", port))
                .await
                .unwrap();
            drop(
                tokio::net::TcpStream::connect(("127.0.0.1", port))
                    .await
                    .unwrap(),
            );
            let mut other = tokio::net::TcpStream::connect(("127.0.0.1", port))
                .await
                .unwrap();
            other
                .write_all(b"GET /favicon.ico HTTP/1.1\r\n\r\n")
                .await
                .unwrap();
            let mut callback = tokio::net::TcpStream::connect(("127.0.0.1", port))
                .await
                .unwrap();
            callback
                .write_all(b"GET /callback?code=c&state=s HTTP/1.1\r\nHost: x\r\n\r\n")
                .await
                .unwrap();
            let back = tokio::time::timeout(std::time::Duration::from_secs(15), waiting)
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            assert_eq!(
                back,
                format!("http://127.0.0.1:{port}/callback?code=c&state=s")
            );
        });
    }

    #[test]
    fn the_first_future_ready_wins() {
        block_on(async {
            let never = std::future::pending::<u8>();
            assert_eq!(super::first(never, async { 2 }).await, 2);
            assert_eq!(super::first(async { 1 }, async { 2 }).await, 1);
        });
    }
}
