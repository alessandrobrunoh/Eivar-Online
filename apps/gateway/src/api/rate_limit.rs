//! Per-client throttling for the endpoints that are expensive to guess at.
//!
//! `/v1/auth/login` and `/v1/auth/register` are asymmetric in the attacker's
//! favour: one HTTP request costs the caller nothing and costs the server an
//! Argon2id hash at 12 MiB (see the module's `reducers::account`) plus a fresh
//! WebSocket to SpacetimeDB. Unthrottled, that is both an online password
//! guessing oracle and a cheap way to exhaust the box.
//!
//! Deliberately hand-rolled rather than `tower_governor`: the maintained crate
//! tracks axum 0.8 while this gateway is on 0.7, and pulling in a second axum
//! would not compile against this `Router`. The policy needed here is one fixed
//! window per client, which is small enough to own and to test.
//!
//! # Fixed window, not a token bucket
//!
//! A fixed window lets a caller spend the whole allowance at the very end of
//! one window and again at the start of the next — a 2x burst across the
//! boundary. For "slow down a password guesser" that is irrelevant: the bound
//! that matters is attempts per hour, and it holds. A token bucket would smooth
//! the burst at the cost of more state per client.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::{ConnectInfo, Request, State};
use axum::http::HeaderMap;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::api::error::AppError;
use crate::AppState;

/// Attempts one client may make against a limited route per [`WINDOW`].
///
/// Generous for a person — a mistyped password three times in a row, then a
/// password manager, is nowhere near it — and ruinous for a guesser: 10 an
/// hour against Argon2id is not an attack, it is a rounding error.
pub const MAX_ATTEMPTS_PER_WINDOW: u32 = 10;

/// Length of one counting window.
pub const WINDOW: Duration = Duration::from_secs(5 * 60);

/// How often stale client entries are swept.
///
/// Without this the map is an unbounded, attacker-controlled allocation: one
/// entry per source address, forever.
const REAP_INTERVAL: Duration = Duration::from_secs(10 * 60);

#[derive(Clone, Copy)]
struct Window {
    started: Instant,
    attempts: u32,
}

/// Fixed-window attempt counter, keyed by client address.
///
/// Cloning shares the same counters (via `Arc`), which is what letting every
/// axum handler hold its own copy of the state wants.
#[derive(Clone, Default)]
pub struct RateLimiter {
    windows: Arc<Mutex<HashMap<IpAddr, Window>>>,
}

impl RateLimiter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Records an attempt by `client` and reports whether it is allowed.
    ///
    /// Takes `now` rather than reading the clock so the window arithmetic is
    /// testable without sleeping.
    fn check_at(&self, client: IpAddr, now: Instant) -> bool {
        let mut windows = self
            .windows
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let window = windows.entry(client).or_insert(Window {
            started: now,
            attempts: 0,
        });

        // `duration_since` rather than `elapsed`: `now` is the caller's clock.
        if now.duration_since(window.started) >= WINDOW {
            *window = Window {
                started: now,
                attempts: 0,
            };
        }

        // Counted before the verdict, so a caller who keeps hammering a closed
        // window does not get a free attempt the moment it rolls over.
        window.attempts = window.attempts.saturating_add(1);
        window.attempts <= MAX_ATTEMPTS_PER_WINDOW
    }

    /// Records an attempt by `client` and reports whether it is allowed.
    pub fn check(&self, client: IpAddr) -> bool {
        self.check_at(client, Instant::now())
    }

    /// Drops clients whose window has fully expired.
    fn reap_at(&self, now: Instant) {
        let mut windows = self
            .windows
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        windows.retain(|_, window| now.duration_since(window.started) < WINDOW);
    }

    /// Spawns the periodic sweep. Call once, from `main`.
    pub fn spawn_reaper(&self) {
        let limiter = self.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(REAP_INTERVAL);
            loop {
                interval.tick().await;
                limiter.reap_at(Instant::now());
            }
        });
    }
}

/// The address to charge this request to.
///
/// With no reverse proxy, that is the socket's peer address, and
/// `X-Forwarded-For` is ignored — anyone can set it, so honouring it would
/// turn the limiter off for whoever bothered to.
///
/// With `trust_proxy` on, the value comes from `X-Forwarded-For`, taking the
/// **last** entry rather than the first. Each hop *appends* the address it
/// received the connection from, so with one trusted proxy in front the header
/// reads `<whatever the client claimed>, <what our proxy actually saw>`. The
/// first entry is client-controlled; the last is the only one our own proxy
/// wrote. Behind two or more hops this would need to skip that many from the
/// right; the deployment in `docker-compose.yml` has exactly one.
pub fn client_ip(headers: &HeaderMap, peer: SocketAddr, trust_proxy: bool) -> IpAddr {
    if !trust_proxy {
        return peer.ip();
    }
    headers
        .get("x-forwarded-for")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.rsplit(',').next())
        .and_then(|entry| entry.trim().parse::<IpAddr>().ok())
        .unwrap_or_else(|| peer.ip())
}

/// Applies [`RateLimiter`] to whatever it wraps. Mounted on `/v1/auth/*` only:
/// the rest of the surface either needs a session already or is a cached read.
pub async fn limit_attempts(
    State(state): State<AppState>,
    peer: Option<ConnectInfo<SocketAddr>>,
    request: Request,
    next: Next,
) -> Response {
    // `ConnectInfo` is missing only if `main` forgot
    // `into_make_service_with_connect_info`. Fail closed rather than silently
    // serving an unlimited endpoint.
    let Some(ConnectInfo(peer)) = peer else {
        tracing::error!("rate limiter has no peer address; refusing the request");
        return AppError::Internal("rate limiter is misconfigured".to_string()).into_response();
    };

    let client = client_ip(request.headers(), peer, state.trust_proxy_headers);
    if !state.auth_limiter.check(client) {
        tracing::warn!(%client, "auth rate limit tripped");
        return AppError::TooManyRequests.into_response();
    }
    next.run(request).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn ip(last: u8) -> IpAddr {
        IpAddr::from([203, 0, 113, last])
    }

    fn peer() -> SocketAddr {
        SocketAddr::from(([198, 51, 100, 7], 44321))
    }

    fn headers_with(forwarded_for: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-forwarded-for",
            HeaderValue::from_str(forwarded_for).unwrap(),
        );
        headers
    }

    #[test]
    fn without_a_trusted_proxy_the_socket_address_wins() {
        let headers = headers_with("1.2.3.4");
        assert_eq!(client_ip(&headers, peer(), false), peer().ip());
    }

    #[test]
    fn with_a_trusted_proxy_the_last_hop_wins() {
        // The client claimed 1.2.3.4; our proxy appended what it actually saw.
        let headers = headers_with("1.2.3.4, 203.0.113.9");
        assert_eq!(client_ip(&headers, peer(), true), ip(9));
    }

    #[test]
    fn a_spoofed_single_entry_cannot_impersonate_another_client() {
        // One hop, so the single entry *is* our proxy's. The point of the test
        // is the pair below it: spoofing changes nothing when untrusted.
        let headers = headers_with("203.0.113.9");
        assert_eq!(client_ip(&headers, peer(), true), ip(9));
        assert_eq!(client_ip(&headers, peer(), false), peer().ip());
    }

    #[test]
    fn a_malformed_header_falls_back_to_the_socket() {
        let headers = headers_with("not-an-address");
        assert_eq!(client_ip(&headers, peer(), true), peer().ip());
        let headers = headers_with("");
        assert_eq!(client_ip(&headers, peer(), true), peer().ip());
    }

    #[test]
    fn a_missing_header_falls_back_to_the_socket() {
        assert_eq!(client_ip(&HeaderMap::new(), peer(), true), peer().ip());
    }

    #[test]
    fn allows_up_to_the_limit_then_refuses() {
        let limiter = RateLimiter::new();
        let start = Instant::now();
        for attempt in 1..=MAX_ATTEMPTS_PER_WINDOW {
            assert!(
                limiter.check_at(ip(1), start),
                "attempt {attempt} is within the allowance"
            );
        }
        assert!(!limiter.check_at(ip(1), start), "one past the allowance");
    }

    #[test]
    fn clients_are_counted_independently() {
        let limiter = RateLimiter::new();
        let start = Instant::now();
        for _ in 0..MAX_ATTEMPTS_PER_WINDOW {
            assert!(limiter.check_at(ip(1), start));
        }
        assert!(!limiter.check_at(ip(1), start));
        // A different address must not inherit the first one's exhausted window.
        assert!(limiter.check_at(ip(2), start));
    }

    #[test]
    fn the_window_rolls_over() {
        let limiter = RateLimiter::new();
        let start = Instant::now();
        for _ in 0..MAX_ATTEMPTS_PER_WINDOW {
            limiter.check_at(ip(1), start);
        }
        assert!(!limiter.check_at(ip(1), start));
        assert!(limiter.check_at(ip(1), start + WINDOW));
    }

    #[test]
    fn hammering_a_closed_window_does_not_bank_a_free_attempt() {
        let limiter = RateLimiter::new();
        let start = Instant::now();
        for _ in 0..MAX_ATTEMPTS_PER_WINDOW * 3 {
            limiter.check_at(ip(1), start);
        }
        // The window rolls over to a fresh allowance, not to a negative debt
        // and not to an immediate refusal.
        assert!(limiter.check_at(ip(1), start + WINDOW));
    }

    #[test]
    fn just_short_of_the_window_still_counts_against_the_allowance() {
        let limiter = RateLimiter::new();
        let start = Instant::now();
        for _ in 0..MAX_ATTEMPTS_PER_WINDOW {
            limiter.check_at(ip(1), start);
        }
        assert!(!limiter.check_at(ip(1), start + WINDOW - Duration::from_millis(1)));
    }

    #[test]
    fn reaping_forgets_expired_clients_only() {
        let limiter = RateLimiter::new();
        let start = Instant::now();
        limiter.check_at(ip(1), start);
        limiter.check_at(ip(2), start + WINDOW);

        limiter.reap_at(start + WINDOW);
        let windows = limiter.windows.lock().unwrap();
        assert!(!windows.contains_key(&ip(1)), "expired client is dropped");
        assert!(windows.contains_key(&ip(2)), "live client is kept");
    }

    #[test]
    fn a_reaped_client_starts_from_a_full_allowance() {
        let limiter = RateLimiter::new();
        let start = Instant::now();
        for _ in 0..MAX_ATTEMPTS_PER_WINDOW {
            limiter.check_at(ip(1), start);
        }
        limiter.reap_at(start + WINDOW);
        assert!(limiter.check_at(ip(1), start + WINDOW));
    }
}
