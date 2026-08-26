//! `/auth/*` and `/profile` — the HTTP surface `apps/frontend` uses instead
//! of talking to SpacetimeDB directly.
//!
//! Every handler here is a thin translation: validate the shape of the
//! request, drive a [`GatewayConnection`], translate its result into an
//! [`AppError`] or a small JSON body. The actual rules (email format, password
//! policy, uniqueness, ownership) live in the SpacetimeDB module's
//! `reducers::account` — this layer never re-implements them, only reports
//! what the module decided.

use axum::extract::State;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::middleware;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Json;
use axum::Router;
use serde::{Deserialize, Serialize};

use crate::api::api_keys::hash_api_key;
use crate::api::error::AppError;
use crate::stdb::connection::{CharacterSummary, GatewayConnection};
use crate::AppState;

const SESSION_COOKIE_NAME: &str = "bevymmo_session";

/// How long the cookie itself persists in the browser. Deliberately longer
/// than [`crate::stdb::session::SessionStore`]'s server-side idle timeout
/// (30 minutes): the two are independent expirations by design. A cookie
/// surviving a week is a normal "stay signed in"; the connection behind it
/// still gets reaped after half an hour of inactivity, at which point
/// `/profile` reports `401` and the frontend sends the user back to
/// `/login` — an explicit re-authentication, not a silent extension.
const SESSION_COOKIE_MAX_AGE_SECS: i64 = 7 * 24 * 60 * 60;

#[derive(Deserialize, utoipa::ToSchema)]
pub struct AuthRequest {
    pub email: String,
    pub password: String,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct ProfileResponse {
    account_id: u64,
    characters: Vec<CharacterSummary>,
}

/// Takes `state` rather than being state-agnostic like the other routers: the
/// rate limiter is `middleware::from_fn_with_state`, which needs the value up
/// front, not the `Router<AppState>` placeholder.
pub fn router(state: AppState) -> Router<AppState> {
    // Only `register` and `login` are throttled, via `route_layer` so the limit
    // applies to these two paths and not to anything merged alongside them.
    // `logout` and `profile` need a cookie the caller cannot guess, so a limit
    // there would cost a browser polling `/profile` without costing an attacker
    // anything.
    let guessable = Router::new()
        .route("/v1/auth/register", post(register))
        .route("/v1/auth/login", post(login))
        .route_layer(middleware::from_fn_with_state(
            state,
            crate::api::rate_limit::limit_attempts,
        ));

    Router::new()
        .merge(guessable)
        .route("/v1/auth/logout", post(logout))
        .route("/v1/profile", get(profile))
}

/// Creates a new account and authenticates the caller as it. Sets the session
/// cookie on success.
#[utoipa::path(
    post,
    tag = "auth",
    path = "/v1/auth/register",
    request_body = AuthRequest,
    responses(
        (status = 200, description = "Account created and logged in; session cookie set", body = ProfileResponse),
        (status = 400, description = "Rejected by the module (email taken, bad format, weak password)", body = crate::api::error::ErrorResponse),
        (status = 502, description = "SpacetimeDB unreachable", body = crate::api::error::ErrorResponse),
    ),
)]
pub async fn register(
    State(state): State<AppState>,
    Json(body): Json<AuthRequest>,
) -> Result<Response, AppError> {
    let connection = open_connection(&state).await?;
    if let Err(reason) = connection.register(body.email, body.password).await {
        connection.disconnect();
        return Err(AppError::BadRequest(reason));
    }
    authenticated_response(&state, connection).await
}

/// Authenticates the caller as an existing account. Sets the session cookie
/// on success.
#[utoipa::path(
    post,
    tag = "auth",
    path = "/v1/auth/login",
    request_body = AuthRequest,
    responses(
        (status = 200, description = "Logged in; session cookie set", body = ProfileResponse),
        (status = 400, description = "Invalid email or password", body = crate::api::error::ErrorResponse),
        (status = 502, description = "SpacetimeDB unreachable", body = crate::api::error::ErrorResponse),
    ),
)]
pub async fn login(
    State(state): State<AppState>,
    Json(body): Json<AuthRequest>,
) -> Result<Response, AppError> {
    let connection = open_connection(&state).await?;
    if let Err(reason) = connection.login(body.email, body.password).await {
        connection.disconnect();
        return Err(AppError::BadRequest(reason));
    }
    authenticated_response(&state, connection).await
}

/// Ends the session behind the caller's cookie and clears the cookie.
#[utoipa::path(
    post,
    tag = "auth",
    path = "/v1/auth/logout",
    responses((status = 204, description = "Session ended; cookie cleared")),
)]
pub async fn logout(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Some(id) = session_id_from_cookie(&headers) {
        state.sessions.end(&id).await;
    }
    let mut response = StatusCode::NO_CONTENT.into_response();
    response
        .headers_mut()
        .insert(header::SET_COOKIE, clear_cookie(state.cookie_secure));
    response
}

/// The caller's own account and character roster, from their session cookie.
#[utoipa::path(
    get,
    tag = "auth",
    path = "/v1/profile",
    responses(
        (status = 200, description = "The authenticated account", body = ProfileResponse),
        (status = 401, description = "No session, or the session expired", body = crate::api::error::ErrorResponse),
    ),
)]
pub async fn profile(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<ProfileResponse>, AppError> {
    let connection = resolve_connection(&state, &headers, true).await?;
    let Some(account_id) = connection.account_id() else {
        return Err(AppError::Unauthorized);
    };
    Ok(Json(ProfileResponse {
        account_id,
        characters: connection.characters().unwrap_or_default(),
    }))
}

/// Opens a fresh SpacetimeDB connection for a `register`/`login` attempt.
/// Not yet stored in the session store or cookied — that only happens once
/// the reducer call on it actually succeeds, so a failed attempt never hands
/// the browser a session id for a connection nobody can use.
/// Cookie session if present, otherwise a Bearer API key when
/// `allow_api_key` is true. Cookie always wins: the Angular site sends
/// `withCredentials` on every call, and management routes pass `false` so a
/// stolen key cannot mint more keys.
pub(crate) async fn resolve_connection(
    state: &AppState,
    headers: &HeaderMap,
    allow_api_key: bool,
) -> Result<GatewayConnection, AppError> {
    if session_id_from_cookie(headers).is_some() {
        return connection_from_cookie(state, headers).await;
    }
    if allow_api_key {
        if let Some(secret) = bearer_token(headers) {
            return connection_from_api_key(state, secret).await;
        }
    }
    Err(AppError::Unauthorized)
}

pub(crate) async fn connection_from_cookie(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<GatewayConnection, AppError> {
    let Some(id) = session_id_from_cookie(headers) else {
        return Err(AppError::Unauthorized);
    };
    let Some(connection) = state.sessions.get(&id).await else {
        return Err(AppError::SessionExpired);
    };
    if connection.account_id().is_none() {
        return Err(AppError::Unauthorized);
    }
    Ok(connection)
}

pub(crate) fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?.trim();
    let token = value
        .strip_prefix("Bearer ")
        .or_else(|| value.strip_prefix("bearer "))?
        .trim();
    if token.is_empty() {
        None
    } else {
        Some(token)
    }
}

fn api_key_session_id(secret: &str) -> String {
    format!("ak:{}", hash_api_key(secret))
}

async fn connection_from_api_key(
    state: &AppState,
    secret: &str,
) -> Result<GatewayConnection, AppError> {
    let cache_id = api_key_session_id(secret);
    if let Some(connection) = state.sessions.get(&cache_id).await {
        // Re-run authenticate so a revoked key dies even if the socket is
        // still cached, and so `last_used_at` moves on every request.
        match connection.authenticate_api_key(secret.to_string()).await {
            Ok(()) => return Ok(connection),
            Err(_) => {
                state.sessions.end(&cache_id).await;
                return Err(AppError::Unauthorized);
            }
        }
    }

    let connection = open_connection(state).await?;
    if let Err(reason) = connection.authenticate_api_key(secret.to_string()).await {
        connection.disconnect();
        tracing::debug!(prefix = %crate::api::api_keys::display_prefix(secret), "api key auth rejected: {reason}");
        return Err(AppError::Unauthorized);
    }
    state.sessions.insert(cache_id, connection.clone()).await;
    Ok(connection)
}

pub(crate) async fn open_connection(state: &AppState) -> Result<GatewayConnection, AppError> {
    GatewayConnection::connect(&state.spacetime_uri, &state.spacetime_module)
        .await
        .map_err(|err| {
            tracing::error!("gateway could not reach SpacetimeDB: {err}");
            AppError::BadGateway
        })
}

/// Common tail of `register`/`login`: the reducer call already succeeded on
/// `connection`, so mint a session id for it, cookie the response, and
/// return the same profile shape `/profile` would.
async fn authenticated_response(
    state: &AppState,
    connection: GatewayConnection,
) -> Result<Response, AppError> {
    let account_id = connection.account_id();
    let characters = connection.characters().unwrap_or_default();
    let id = state.sessions.create(connection).await;

    let mut response = match account_id {
        Some(account_id) => Json(ProfileResponse {
            account_id,
            characters,
        })
        .into_response(),
        // Should not happen — the reducer call just returned `Ok`, which
        // only happens after `bind_session` writes the `Session` row this
        // reads back — but fail closed with a real status rather than
        // panicking on the `unwrap` that would otherwise be tempting here.
        None => {
            return Err(AppError::Internal(
                "authenticated but no session was recorded".to_string(),
            ))
        }
    };
    response
        .headers_mut()
        .insert(header::SET_COOKIE, session_cookie(&id, state.cookie_secure));
    Ok(response)
}

/// The session id on the request cookie, if any. Shared with the
/// authenticated character routes that reuse the same cookie.
pub(crate) fn session_id_from_cookie(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get(header::COOKIE)?.to_str().ok()?;
    raw.split(';').find_map(|pair| {
        let (name, value) = pair.trim().split_once('=')?;
        (name == SESSION_COOKIE_NAME).then(|| value.to_string())
    })
}

fn session_cookie(id: &str, secure: bool) -> HeaderValue {
    let secure_attr = if secure { "; Secure" } else { "" };
    let value = format!(
        "{SESSION_COOKIE_NAME}={id}; Path=/; HttpOnly; SameSite=Lax; Max-Age={SESSION_COOKIE_MAX_AGE_SECS}{secure_attr}"
    );
    HeaderValue::from_str(&value).expect("cookie header value is always valid ASCII")
}

fn clear_cookie(secure: bool) -> HeaderValue {
    let secure_attr = if secure { "; Secure" } else { "" };
    let value =
        format!("{SESSION_COOKIE_NAME}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0{secure_attr}");
    HeaderValue::from_str(&value).expect("cookie header value is always valid ASCII")
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn headers_with(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for (name, value) in pairs {
            headers.append(*name, HeaderValue::from_static(value));
        }
        headers
    }

    #[test]
    fn bearer_token_reads_standard_authorization() {
        let headers = headers_with(&[("authorization", "Bearer eiv_abc")]);
        assert_eq!(bearer_token(&headers), Some("eiv_abc"));
    }

    #[test]
    fn bearer_token_is_case_insensitive_on_the_scheme() {
        let headers = headers_with(&[("authorization", "bearer eiv_abc")]);
        assert_eq!(bearer_token(&headers), Some("eiv_abc"));
    }

    #[test]
    fn bearer_token_ignores_empty_and_missing() {
        assert!(bearer_token(&HeaderMap::new()).is_none());
        let headers = headers_with(&[("authorization", "Bearer ")]);
        assert!(bearer_token(&headers).is_none());
        let headers = headers_with(&[("authorization", "Basic abc")]);
        assert!(bearer_token(&headers).is_none());
    }

    #[test]
    fn cookie_is_detected_independently_of_bearer() {
        let headers = headers_with(&[
            ("cookie", "bevymmo_session=abc-123"),
            ("authorization", "Bearer eiv_abc"),
        ]);
        assert_eq!(session_id_from_cookie(&headers).as_deref(), Some("abc-123"));
        assert_eq!(bearer_token(&headers), Some("eiv_abc"));
    }
}
