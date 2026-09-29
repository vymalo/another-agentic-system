use std::future::Future;
use std::time::Duration;

/// How long tests wait for something that should happen "soon".
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(20);

/// Polls `f` every 10 ms until it returns `Some`, or panics after [`DEFAULT_TIMEOUT`].
pub async fn eventually<T, F, Fut>(what: &str, f: F) -> T
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Option<T>>,
{
    eventually_within(DEFAULT_TIMEOUT, what, f).await
}

/// Like [`eventually`] with an explicit deadline.
pub async fn eventually_within<T, F, Fut>(within: Duration, what: &str, mut f: F) -> T
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Option<T>>,
{
    let deadline = tokio::time::Instant::now() + within;
    loop {
        if let Some(v) = f().await {
            return v;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out after {within:?} waiting for: {what}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}
