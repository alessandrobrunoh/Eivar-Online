//! The gateway's single error type and the only place an error becomes an
//! HTTP response.
//!
//! Every handler returns `Result<T, AppError>`, so status codes cannot drift
//! between endpoints and the body is always the same flat JSON the frontend
//! already parses. The mapping follows standard HTTP semantics:
//!
//! | variant | status | meaning |
//! |---|---|---|
//! | `BadRequest` | 400 | the module rejected the request (email taken, weak password, ...) |
//! | `Unauthorized` | 401 | no session on a route that needs one |
//! | `SessionExpired` | 401 | cookie present, but its server-side connection was reaped |
//! | `Forbidden` | 403 | authenticated, but the resource belongs to another account |
//! | `NotFound` | 404 | no such character, or no such route (the fallback) |
//! | `TooManyRequests` | 429 | the caller tripped the auth rate limit |
//! | `BadGateway` | 502 | SpacetimeDB unreachable while opening an auth connection |
//! | `ServiceUnavailable` | 503 | the shared directory connection is down; retry later |
//! | `Timeout` | 504 | the request exceeded [`crate::api::REQUEST_TIMEOUT`] |
//! | `Internal` | 500 | a bug — logged with detail, reported generically |
//!
//! Not RFC 9457 `application/problem+json` on purpose: it would be a
//! coordinated change across `apps/frontend` for no gain at this size.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;
use thiserror::Error;

#[derive(Debug, Error)]
pub(crate) enum AppError {
    #[error("{0}")]
    BadRequest(String),
    #[error("not authenticated")]
    Unauthorized,
    #[error("session expired")]
    SessionExpired,
    #[error("{0}")]
    Forbidden(String),
    #[error("{0}")]
    NotFound(String),
    #[error("too many attempts, try again in a moment")]
    TooManyRequests,
    #[error("could not reach the game server")]
    BadGateway,
    #[error("could not reach the game server, try again later")]
    ServiceUnavailable,
    #[error("the game server did not respond in time")]
    Timeout,
    #[error("{0}")]
    Internal(String),
}

impl AppError {
    fn status(&self) -> StatusCode {
        match self {
            Self::BadRequest(_) => StatusCode::BAD_REQUEST,
            Self::Unauthorized | Self::SessionExpired => StatusCode::UNAUTHORIZED,
            Self::Forbidden(_) => StatusCode::FORBIDDEN,
            Self::NotFound(_) => StatusCode::NOT_FOUND,
            Self::TooManyRequests => StatusCode::TOO_MANY_REQUESTS,
            Self::BadGateway => StatusCode::BAD_GATEWAY,
            Self::ServiceUnavailable => StatusCode::SERVICE_UNAVAILABLE,
            Self::Timeout => StatusCode::GATEWAY_TIMEOUT,
            Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = self.status();
        // 5xx means the gateway or its upstream is at fault — always worth a
        // server-side log line; 4xx is the client's business.
        if status.is_server_error() {
            tracing::error!(status = %status, "request failed: {self}");
        }
        (
            status,
            Json(ErrorResponse {
                error: self.to_string(),
            }),
        )
            .into_response()
    }
}

/// The error body every endpoint returns on failure.
#[derive(Serialize, utoipa::ToSchema)]
pub(crate) struct ErrorResponse {
    pub(crate) error: String,
}
