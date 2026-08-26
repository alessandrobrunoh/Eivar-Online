//! The gateway's HTTP surface, one module per API area.
//!
//! - [`auth`]: `/v1/auth/*` and `/v1/profile`, cookie or Bearer API key.
//! - [`api_keys`]: `/v1/api-keys`, cookie only — create/list/revoke.
//! - [`characters`]: `/v1/characters/:id/{wallet,stats}`, cookie or Bearer.
//! - [`public`]: `/v1/public/*`, no session required — live module rows
//!   (markets, accounts) and the compiled game catalog.
//! - [`docs`]: the Scalar API reference at `/docs`.
//! - [`error`]: [`AppError`], the single error type every handler returns.
//!
//! Versioning: every gameplay/business route lives under `/v1`. `/`,
//! `/health` and `/docs` stay unversioned on purpose — they are about the
//! *service*, not the API contract, and pinning them to a version would force
//! a probe/config change on every bump. Breaking change → add `/v2` routes
//! alongside, deprecate `/v1` in the Scalar description.
//!
//! Every module exposes a `router()` returning its own routes; [`router`]
//! merges them and wraps the result in the shared middleware stack (see the
//! constants and helpers below it). Handlers stay thin translation layers —
//! rules live in the SpacetimeDB module, connection plumbing in [`crate::stdb`].

pub mod api_keys;
pub mod auth;
pub mod characters;
pub mod docs;
pub mod error;
pub mod public;
pub(crate) mod rate_limit;

use std::time::{Duration, Instant};

use axum::error_handling::HandleErrorLayer;
use axum::extract::{Request, State};
use axum::http::{HeaderValue, Method, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Json;
use axum::Router;
use serde::Serialize;
use tower::ServiceBuilder;
use tower_http::catch_panic::CatchPanicLayer;
use uuid::Uuid;

use crate::api::error::AppError;
use crate::AppState;

/// Hard upper bound for any one request, SpacetimeDB round-trips included. A
/// hung upstream must fail fast (504) instead of pinning connections until
/// each client gives up on its own. Generous rather than tight: opening a
/// session connection includes a subscription handshake.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// Header used to correlate a client report with the gateway's log line.
/// Reused if the incoming request already carries one (e.g. from a reverse
/// proxy), minted otherwise.
const REQUEST_ID_HEADER: &str = "x-request-id";

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(welcome))
        .route("/health", get(health))
        .merge(auth::router(state.clone()))
        .merge(api_keys::router())
        .merge(characters::router())
        .merge(public::router())
        .merge(docs::router())
        // Unmatched paths get the same JSON error body as everything else,
        // instead of axum's empty 404.
        .fallback(not_found)
        .layer(
            ServiceBuilder::new()
                // `HandleErrorLayer` must wrap the timeout: the timeout is
                // the one middleware whose error type is not `Infallible`,
                // and this is where that error becomes a response. In a
                // `ServiceBuilder`, earlier layers are outermost.
                .layer(HandleErrorLayer::new(handle_middleware_error))
                .timeout(REQUEST_TIMEOUT),
        )
        // A panicking handler becomes a logged 500, not a dropped connection.
        .layer(CatchPanicLayer::custom(panic_response))
        // Outermost, so panics and timeouts inside are logged too: request
        // id + one access-log line per request.
        .layer(middleware::from_fn(observe_request))
        .with_state(state)
}

#[derive(Serialize, utoipa::ToSchema)]
pub(crate) struct WelcomeResponse {
    pub(crate) message: &'static str,
    pub(crate) service: &'static str,
    /// The SpacetimeDB module name the gateway is wired to. Useful to confirm
    /// the gateway and the desktop client are pointing at the same world.
    pub(crate) spacetime_module: String,
}

#[utoipa::path(
    get,
    tag = "meta",
    path = "/",
    responses((status = 200, description = "Service banner", body = WelcomeResponse)),
)]
pub(crate) async fn welcome(State(state): State<AppState>) -> Json<WelcomeResponse> {
    Json(WelcomeResponse {
        message: "Welcome to the BevyMMO gateway",
        service: "bevymmo_gateway",
        spacetime_module: state.spacetime_module,
    })
}

#[derive(Serialize, utoipa::ToSchema)]
pub(crate) struct HealthResponse {
    pub(crate) status: &'static str,
    pub(crate) service: &'static str,
}

#[utoipa::path(
    get,
    tag = "meta",
    path = "/health",
    responses((status = 200, description = "Liveness probe", body = HealthResponse)),
)]
pub(crate) async fn health() -> impl IntoResponse {
    (
        StatusCode::OK,
        Json(HealthResponse {
            status: "ok",
            service: "bevymmo_gateway",
        }),
    )
}

/// Router fallback: unmatched paths, JSON-shaped like every other error.
async fn not_found() -> AppError {
    AppError::NotFound("route not found".to_string())
}

/// See [`router`]. The only fallible middleware inside `HandleErrorLayer` is
/// the timeout, so any error reaching here is one — hence 504 (upstream too
/// slow), the gateway-correct code, not 500 (our bug).
async fn handle_middleware_error(err: axum::BoxError) -> Response {
    tracing::error!("request middleware error: {err}");
    AppError::Timeout.into_response()
}

/// `CatchPanicLayer::custom`: log the payload server-side, tell the client
/// nothing beyond "internal error" — panic messages are not for the wire.
fn panic_response(panic: Box<dyn std::any::Any + Send + 'static>) -> Response {
    let detail = panic
        .downcast_ref::<&str>()
        .map(|s| (*s).to_string())
        .or_else(|| panic.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic payload".to_string());
    tracing::error!("handler panicked: {detail}");
    AppError::Internal("internal server error".to_string()).into_response()
}

/// Outermost middleware: stamps an `x-request-id` on request and response,
/// and writes one access-log line (method, path, status, latency, id) so any
/// client-reported problem is findable in the logs.
async fn observe_request(mut request: Request, next: Next) -> Response {
    let started = Instant::now();
    let method = request.method().clone();
    let path = request.uri().path().to_owned();

    let request_id = request
        .headers()
        .get(REQUEST_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    // ASCII by construction: header values are, and so is a UUID.
    let request_id_value = HeaderValue::from_str(&request_id).expect("request id is valid ASCII");
    request
        .headers_mut()
        .insert(REQUEST_ID_HEADER, request_id_value.clone());

    let mut response = next.run(request).await;
    response
        .headers_mut()
        .insert(REQUEST_ID_HEADER, request_id_value);

    let elapsed_ms = started.elapsed().as_millis() as u64;
    // Health checks and CORS preflights are routine noise; keep them out of
    // info-level logs.
    let quiet = path == "/health" || method == Method::OPTIONS;
    if quiet {
        tracing::debug!(
            request_id = %request_id,
            %method,
            %path,
            status = %response.status().as_u16(),
            elapsed_ms,
            "request"
        );
    } else {
        tracing::info!(
            request_id = %request_id,
            %method,
            %path,
            status = %response.status().as_u16(),
            elapsed_ms,
            "request"
        );
    }
    response
}
