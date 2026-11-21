//! What a pool's connections say to the server when they end. Skipped unless
//! `ORCH_TEST_DATABASE_URL` names a plain (no TLS) server: a forwarder between the pool and it
//! records what each connection sent.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use orch_pg_connect::{ConnectionTasks, LINGER, PgOptions};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

/// What one connection sent, and whether it has ended its side.
#[derive(Default)]
struct Sent {
    bytes: Vec<u8>,
    ended: bool,
}

type Seen = Arc<Mutex<Vec<Arc<Mutex<Sent>>>>>;

/// A forwarder on a port of its own to `target`, recording what every client sends.
async fn recorder(target: String) -> (u16, Seen) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen: Seen = Arc::default();
    let all = seen.clone();
    tokio::spawn(async move {
        while let Ok((inbound, _)) = listener.accept().await {
            let sent = Arc::new(Mutex::new(Sent::default()));
            all.lock().unwrap().push(sent.clone());
            let outbound = tokio::net::TcpStream::connect(&target).await.unwrap();
            let (mut from_client, mut to_client) = inbound.into_split();
            let (mut from_server, mut to_server) = outbound.into_split();
            tokio::spawn(async move {
                let _ = tokio::io::copy(&mut from_server, &mut to_client).await;
            });
            tokio::spawn(async move {
                let mut buf = [0_u8; 8192];
                loop {
                    let n = from_client.read(&mut buf).await.unwrap_or(0);
                    if n == 0 {
                        sent.lock().unwrap().ended = true;
                        let _ = to_server.shutdown().await;
                        return;
                    }
                    sent.lock().unwrap().bytes.extend_from_slice(&buf[..n]);
                    if to_server.write_all(&buf[..n]).await.is_err() {
                        sent.lock().unwrap().ended = true;
                        return;
                    }
                }
            });
        }
    });
    (port, seen)
}

/// The frontend's `Terminate`: `X` and a length of 4.
const TERMINATE: [u8; 5] = [b'X', 0, 0, 0, 4];

/// A pool built and used on a runtime of its own, closed, its connection tasks waited for, and
/// that runtime dropped at once, as a process that exits drops it: every connection told the
/// server it was leaving. (`deadpool-postgres`' own connector aborts the task: no `Terminate`.)
#[test]
fn a_closed_pool_s_connections_end_with_terminate() {
    let Ok(url) = std::env::var("ORCH_TEST_DATABASE_URL") else {
        eprintln!("skipped: ORCH_TEST_DATABASE_URL unset");
        return;
    };
    let mut url = url::Url::parse(&url).unwrap();
    let target = format!(
        "{}:{}",
        url.host_str().unwrap_or("localhost"),
        url.port().unwrap_or(5432)
    );
    let outer = tokio::runtime::Runtime::new().unwrap();
    let (port, seen) = outer.block_on(recorder(target));
    url.set_host(Some("127.0.0.1")).unwrap();
    url.set_port(Some(port)).unwrap();
    url.query_pairs_mut().append_pair("sslmode", "disable");
    let options: PgOptions = url.as_str().parse().unwrap();

    let inner = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    inner.block_on(async {
        let tasks = ConnectionTasks::default();
        let pool = options
            .pool_tracked(2, Duration::from_secs(5), &tasks)
            .unwrap();
        let (a, b) = tokio::join!(pool.get(), pool.get());
        let (a, b) = (a.unwrap(), b.unwrap());
        a.simple_query("SELECT 1").await.unwrap();
        b.simple_query("SELECT 1").await.unwrap();
        drop((a, b));
        assert_eq!(tasks.running(), 2);
        pool.close();
        assert!(tasks.ended_within(LINGER).await, "the connections ended");
    });
    drop(inner);

    let ended = |seen: &Seen| {
        let all = seen.lock().unwrap();
        all.len() == 2 && all.iter().all(|s| s.lock().unwrap().ended)
    };
    let waited = outer.block_on(async {
        tokio::time::timeout(Duration::from_secs(5), async {
            while !ended(&seen) {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
    });
    assert!(waited.is_ok(), "the connections never ended");
    for sent in seen.lock().unwrap().iter() {
        let sent = sent.lock().unwrap();
        assert!(
            sent.bytes.ends_with(&TERMINATE),
            "a connection ended without Terminate: {:?}",
            &sent.bytes[sent.bytes.len().saturating_sub(16)..]
        );
    }
}
