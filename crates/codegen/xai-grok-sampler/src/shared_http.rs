// Modified in 2026 by the ocque41 OpenAI-support fork; see FORK-NOTICE.md.
//! Process-wide shared `reqwest::Client`s for sampling requests.
//!
//! Sharing one client across all `SamplingClient` instances is safe because
//! the builders below take no config-derived input: auth, extra headers, base
//! URL, and User-Agent are all applied per-request in `SamplingClient::post`.
//! Stale-connection exposure is bounded by HTTP/2 keepalive pings (15s
//! interval, 5s timeout, while idle), the 90s idle-pool eviction, and the
//! first-retry HTTP/1.1 rebuild escape hatch (that client never pools, so
//! every use opens a fresh connection).
//!
//! Wire-level behavior (connection reuse, header isolation, pool-less http1
//! fallback, same-origin-only redirects, kill switch) is pinned by the
//! `shared_http_wire` and
//! `shared_http_kill_switch` integration binaries, which own their process
//! environment.

use std::sync::OnceLock;
use std::time::Duration;

use reqwest::redirect::Policy;

static SHARED_H2: OnceLock<reqwest::Client> = OnceLock::new();
static SHARED_HTTP1: OnceLock<reqwest::Client> = OnceLock::new();

const MAX_SAME_ORIGIN_REDIRECTS: usize = 10;

fn urls_share_origin(previous: &reqwest::Url, next: &reqwest::Url) -> bool {
    previous.scheme() == next.scheme()
        && previous.host_str() == next.host_str()
        && previous.port_or_known_default() == next.port_or_known_default()
}

/// Follow ordinary redirects only while the URL remains on the exact same
/// origin (scheme, host, and effective port). Sampling requests can carry
/// provider credentials, custom headers, and private request bodies; reqwest
/// strips a small fixed set of sensitive headers on cross-host redirects but
/// cannot know that fields such as `x-grok-*` are also private. Stopping at
/// the first cross-origin hop prevents all of that state from being replayed
/// to another provider while retaining same-provider redirect compatibility.
fn same_origin_redirect_policy() -> Policy {
    Policy::custom(|attempt| {
        if attempt.previous().len() > MAX_SAME_ORIGIN_REDIRECTS {
            return attempt.error("too many same-origin redirects");
        }

        let Some(previous) = attempt.previous().last() else {
            return attempt.stop();
        };
        if urls_share_origin(previous, attempt.url()) {
            attempt.follow()
        } else {
            attempt.stop()
        }
    })
}

/// Kill switch: `GROK_SAMPLER_SHARED_CLIENT=0` (or `false`, any case)
/// restores the old behavior of building a fresh `reqwest::Client` per
/// `SamplingClient`. Resolved once per process: the environment cannot
/// change externally after spawn, and latching keeps the rollback state
/// consistent with the read-once pool knobs.
fn sharing_disabled() -> bool {
    static DISABLED: OnceLock<bool> = OnceLock::new();
    *DISABLED.get_or_init(|| {
        let disabled = match std::env::var("GROK_SAMPLER_SHARED_CLIENT") {
            Ok(v) => v == "0" || v.eq_ignore_ascii_case("false"),
            Err(_) => false,
        };
        if disabled {
            tracing::info!("sampler HTTP client sharing disabled via GROK_SAMPLER_SHARED_CLIENT");
        }
        disabled
    })
}

/// Clone the shared client out of `cell`, building it on first use. Build
/// failures are not cached: on `Err` the cell stays empty and the next call
/// retries. A racing loser's freshly built client is simply dropped.
fn shared(
    cell: &OnceLock<reqwest::Client>,
    build: fn() -> Result<reqwest::Client, reqwest::Error>,
    disabled: bool,
) -> Result<reqwest::Client, reqwest::Error> {
    if disabled {
        return build();
    }
    if let Some(client) = cell.get() {
        return Ok(client.clone());
    }
    let built = build()?;
    Ok(cell.get_or_init(|| built).clone())
}

/// Shared HTTP/2 sampling client (connection pooling + h2 keepalive).
pub(crate) fn client() -> Result<reqwest::Client, reqwest::Error> {
    shared(&SHARED_H2, build_http_client, sharing_disabled())
}

/// Shared HTTP/1.1 fallback client. Pool-less by construction, so sharing it
/// is behaviorally identical to building a fresh one.
pub(crate) fn client_http1() -> Result<reqwest::Client, reqwest::Error> {
    shared(&SHARED_HTTP1, build_http_client_http1, sharing_disabled())
}

/// Build a `reqwest::Client` for sampling with HTTP/2 + connection pooling.
/// Env knobs are read once, when the shared client is first built.
fn build_http_client() -> Result<reqwest::Client, reqwest::Error> {
    let pool_max_idle: usize = std::env::var("GROK_POOL_MAX_IDLE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2);
    let pool_idle_timeout_secs: u64 = std::env::var("GROK_POOL_IDLE_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(90);
    let connect_timeout_secs: u64 = std::env::var("GROK_CONNECT_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(10);

    reqwest::Client::builder()
        .redirect(same_origin_redirect_policy())
        .pool_max_idle_per_host(pool_max_idle)
        .pool_idle_timeout(Duration::from_secs(pool_idle_timeout_secs))
        .connect_timeout(Duration::from_secs(connect_timeout_secs))
        .tcp_nodelay(true)
        // HTTP/2 keep-alive: ping every 15s, timeout after 5s.
        .http2_keep_alive_interval(Duration::from_secs(15))
        .http2_keep_alive_timeout(Duration::from_secs(5))
        .http2_keep_alive_while_idle(true)
        .build()
}

/// Build a `reqwest::Client` constrained to HTTP/1.1 with pooling disabled.
/// Used as a fallback after HTTP/2 transport failures.
fn build_http_client_http1() -> Result<reqwest::Client, reqwest::Error> {
    let connect_timeout_secs: u64 = std::env::var("GROK_CONNECT_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(10);

    reqwest::Client::builder()
        .redirect(same_origin_redirect_policy())
        .pool_max_idle_per_host(0)
        .pool_idle_timeout(Duration::from_secs(0))
        .connect_timeout(Duration::from_secs(connect_timeout_secs))
        .tcp_nodelay(true)
        .http1_only()
        .build()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use axum::Router;
    use axum::body::Bytes;
    use axum::extract::State;
    use axum::http::header::LOCATION;
    use axum::http::{HeaderMap, HeaderValue, StatusCode};
    use axum::response::{IntoResponse, Response};
    use tokio::net::TcpListener;

    use super::{build_http_client, build_http_client_http1, shared, urls_share_origin};

    static BUILD_CALLS: AtomicUsize = AtomicUsize::new(0);

    /// Fails on the first call (a real `reqwest::Error`, no I/O), then builds.
    fn flaky_build() -> Result<reqwest::Client, reqwest::Error> {
        if BUILD_CALLS.fetch_add(1, Ordering::SeqCst) == 0 {
            return Err(reqwest::Proxy::all("not a proxy url").unwrap_err());
        }
        reqwest::Client::builder().build()
    }

    #[test]
    fn shared_does_not_cache_build_failures() {
        static CELL: OnceLock<reqwest::Client> = OnceLock::new();
        assert!(shared(&CELL, flaky_build, false).is_err());
        assert!(CELL.get().is_none(), "failure must leave the cell empty");
        assert!(shared(&CELL, flaky_build, false).is_ok());
        assert!(CELL.get().is_some(), "success must populate the cell");
        assert!(shared(&CELL, flaky_build, false).is_ok());
        assert_eq!(
            BUILD_CALLS.load(Ordering::SeqCst),
            2,
            "third call must reuse the cached client, not rebuild"
        );
    }

    #[test]
    fn shared_disabled_bypasses_cell() {
        static CELL: OnceLock<reqwest::Client> = OnceLock::new();
        assert!(shared(&CELL, || reqwest::Client::builder().build(), true).is_ok());
        assert!(
            CELL.get().is_none(),
            "disabled mode must never touch the cell"
        );
    }

    #[test]
    fn redirect_origin_comparison_includes_scheme_host_and_effective_port() {
        let https_default = reqwest::Url::parse("https://api.x.ai/v1").unwrap();
        let https_explicit = reqwest::Url::parse("https://api.x.ai:443/next").unwrap();
        let different_scheme = reqwest::Url::parse("http://api.x.ai/next").unwrap();
        let different_host = reqwest::Url::parse("https://api.openai.com/next").unwrap();
        let different_port = reqwest::Url::parse("https://api.x.ai:444/next").unwrap();

        assert!(urls_share_origin(&https_default, &https_explicit));
        assert!(!urls_share_origin(&https_default, &different_scheme));
        assert!(!urls_share_origin(&https_default, &different_host));
        assert!(!urls_share_origin(&https_default, &different_port));
    }

    async fn count_target_request(
        State(hits): State<Arc<AtomicUsize>>,
        _headers: HeaderMap,
        _body: Bytes,
    ) -> StatusCode {
        hits.fetch_add(1, Ordering::SeqCst);
        StatusCode::OK
    }

    async fn temporary_redirect(State(location): State<String>) -> Response {
        let mut response = StatusCode::TEMPORARY_REDIRECT.into_response();
        response.headers_mut().insert(
            LOCATION,
            HeaderValue::from_str(&location).expect("test redirect URL is a valid header"),
        );
        response
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn shared_clients_never_replay_provider_state_cross_origin() {
        let target_hits = Arc::new(AtomicUsize::new(0));
        let target_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target_addr = target_listener.local_addr().unwrap();
        let target_app = Router::new()
            .fallback(count_target_request)
            .with_state(target_hits.clone());
        let target_task = tokio::spawn(async move {
            axum::serve(target_listener, target_app).await.unwrap();
        });

        let redirect_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let redirect_addr = redirect_listener.local_addr().unwrap();
        let redirect_app = Router::new()
            .fallback(temporary_redirect)
            .with_state(format!("http://{target_addr}/capture"));
        let redirect_task = tokio::spawn(async move {
            axum::serve(redirect_listener, redirect_app).await.unwrap();
        });

        let request_url = format!("http://{redirect_addr}/v1/responses");
        let private_body = serde_json::json!({
            "input": "provider-private-body",
            "stream_tool_calls": true,
            "tools": [{"type": "x_search"}],
        });
        for client in [
            build_http_client().expect("shared HTTP/2 client builds"),
            build_http_client_http1().expect("shared HTTP/1 client builds"),
        ] {
            let response = client
                .post(&request_url)
                .header("authorization", "Bearer openai-private-key")
                .header("x-api-key", "provider-private-key")
                .header("x-grok-conv-id", "xai-private-conversation")
                .header("x-custom-provider-secret", "custom-private-header")
                .json(&private_body)
                .send()
                .await
                .expect("the redirecting origin responds");

            assert_eq!(
                response.status(),
                StatusCode::TEMPORARY_REDIRECT,
                "a cross-origin redirect must be returned without being followed"
            );
        }

        tokio::task::yield_now().await;
        assert_eq!(
            target_hits.load(Ordering::SeqCst),
            0,
            "no xAI, OpenAI, or custom provider headers/body may be replayed cross-origin"
        );

        redirect_task.abort();
        target_task.abort();
    }
}
