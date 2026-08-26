//! `/v1/api-keys` — cookie-only management of the caller's HTTP API keys.
//!
//! Create/list/revoke go through the logged-in browser session. Bots holding
//! a Bearer token cannot mint more keys; that is the point of keeping this
//! router separate from [`crate::api::auth::resolve_connection`].

use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::routing::{delete, get};
use axum::Json;
use axum::Router;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::api::auth::connection_from_cookie;
use crate::api::error::AppError;
use crate::stdb::connection::GatewayConnection;
use crate::stdb::module_bindings::api_key_meta_type::ApiKeyMeta;
use crate::AppState;

const SECRET_PREFIX: &str = "eiv_";
const DISPLAY_PREFIX_LEN: usize = 12;

#[derive(Deserialize, utoipa::ToSchema)]
pub struct CreateApiKeyRequest {
    pub name: String,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct ApiKeyListItem {
    pub id: Uuid,
    pub name: String,
    pub prefix: String,
    /// Unix microseconds, same unit SpacetimeDB stores.
    pub created_at: i64,
    pub last_used_at: Option<i64>,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct CreatedApiKey {
    pub id: Uuid,
    pub name: String,
    pub prefix: String,
    /// Plaintext secret. Present only on create; list never includes it.
    pub key: String,
    pub created_at: i64,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/api-keys", get(list).post(create))
        .route("/v1/api-keys/:id", delete(revoke))
}

#[utoipa::path(
    get,
    tag = "api-keys",
    path = "/v1/api-keys",
    responses(
        (status = 200, description = "The caller's API keys, without secrets", body = Vec<ApiKeyListItem>),
        (status = 401, description = "No session, or the session expired", body = crate::api::error::ErrorResponse),
    )
)]
pub async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<ApiKeyListItem>>, AppError> {
    let connection = connection_from_cookie(&state, &headers).await?;
    Ok(Json(
        connection.api_keys().into_iter().map(list_item).collect(),
    ))
}

#[utoipa::path(
    post,
    tag = "api-keys",
    path = "/v1/api-keys",
    request_body = CreateApiKeyRequest,
    responses(
        (status = 200, description = "Key created; `key` is shown this once", body = CreatedApiKey),
        (status = 400, description = "Rejected by the module (name, cap, ...)", body = crate::api::error::ErrorResponse),
        (status = 401, description = "No session, or the session expired", body = crate::api::error::ErrorResponse),
    )
)]
pub async fn create(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<CreateApiKeyRequest>,
) -> Result<Json<CreatedApiKey>, AppError> {
    let connection = connection_from_cookie(&state, &headers).await?;
    let id = Uuid::new_v4();
    let secret = mint_secret();
    let prefix = display_prefix(&secret);

    if let Err(reason) = connection
        .create_api_key(id, body.name.clone(), prefix.clone(), secret.clone())
        .await
    {
        return Err(AppError::BadRequest(reason));
    }

    let stored = connection.api_key(id);
    let created_at = stored
        .as_ref()
        .map(|row| row.created_at.to_micros_since_unix_epoch())
        .unwrap_or_else(unix_micros_now);
    let name = stored
        .as_ref()
        .map(|row| row.name.clone())
        .unwrap_or(body.name);

    Ok(Json(CreatedApiKey {
        id,
        name,
        prefix,
        key: secret,
        created_at,
    }))
}

#[utoipa::path(
    delete,
    tag = "api-keys",
    path = "/v1/api-keys/{id}",
    params(("id" = Uuid, Path, description = "API key id from GET /v1/api-keys")),
    responses(
        (status = 204, description = "Key revoked"),
        (status = 401, description = "No session, or the session expired", body = crate::api::error::ErrorResponse),
        (status = 403, description = "The key belongs to another account", body = crate::api::error::ErrorResponse),
        (status = 404, description = "No key with that id", body = crate::api::error::ErrorResponse),
    )
)]
pub async fn revoke(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<axum::http::StatusCode, AppError> {
    let connection = connection_from_cookie(&state, &headers).await?;
    match connection.revoke_api_key(id).await {
        Ok(()) => Ok(axum::http::StatusCode::NO_CONTENT),
        Err(reason) if reason.contains("no api key") => Err(AppError::NotFound(reason)),
        Err(reason) if reason.contains("does not belong") => Err(AppError::Forbidden(reason)),
        Err(reason) => Err(AppError::BadRequest(reason)),
    }
}

/// Mints a fresh API key secret: `eiv_` plus 64 lowercase hex characters.
///
/// The 32 bytes come from two v4 UUIDs, so 12 of the 256 bits are the UUID
/// version and variant markers rather than entropy — 244 random bits, not 256.
/// Far beyond what an unguessable token needs; noted because the shape reads
/// like 32 random bytes and is not quite.
pub(crate) fn mint_secret() -> String {
    let bytes = {
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let mut out = [0u8; 32];
        out[..16].copy_from_slice(a.as_bytes());
        out[16..].copy_from_slice(b.as_bytes());
        out
    };
    let mut hex = String::with_capacity(64);
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for &b in &bytes {
        hex.push(HEX[(b >> 4) as usize] as char);
        hex.push(HEX[(b & 0x0f) as usize] as char);
    }
    format!("{SECRET_PREFIX}{hex}")
}

pub(crate) fn display_prefix(secret: &str) -> String {
    secret.chars().take(DISPLAY_PREFIX_LEN).collect()
}

pub(crate) fn hash_api_key(secret: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(secret.as_bytes());
    let mut hex = String::with_capacity(64);
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for &b in digest.as_slice() {
        hex.push(HEX[(b >> 4) as usize] as char);
        hex.push(HEX[(b & 0x0f) as usize] as char);
    }
    hex
}

fn list_item(row: ApiKeyMeta) -> ApiKeyListItem {
    ApiKeyListItem {
        id: Uuid::from_u128(row.id.as_u128()),
        name: row.name,
        prefix: row.prefix,
        created_at: row.created_at.to_micros_since_unix_epoch(),
        last_used_at: row.last_used_at.map(|ts| ts.to_micros_since_unix_epoch()),
    }
}

fn unix_micros_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_micros() as i64)
        .unwrap_or(0)
}

/// Shared ownership check used by wallet/stats: the character exists and
/// belongs to this connection's account.
pub(crate) fn owned_character(
    connection: &GatewayConnection,
    character_id: Uuid,
) -> Result<(), AppError> {
    let Some(account_id) = connection.account_id() else {
        return Err(AppError::Unauthorized);
    };
    let Some(player) = connection.player(character_id) else {
        return Err(AppError::NotFound(format!(
            "no character with id {character_id}"
        )));
    };
    if player.account_id != account_id {
        return Err(AppError::Forbidden(
            "that character does not belong to this account".to_string(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mint_secret_has_the_documented_shape() {
        let secret = mint_secret();
        assert!(secret.starts_with(SECRET_PREFIX), "{secret}");
        assert_eq!(secret.len(), SECRET_PREFIX.len() + 64);
        assert!(secret[SECRET_PREFIX.len()..]
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')));
        let prefix = display_prefix(&secret);
        assert_eq!(prefix.len(), DISPLAY_PREFIX_LEN);
        assert!(secret.starts_with(&prefix));
    }

    #[test]
    fn two_mints_differ() {
        assert_ne!(mint_secret(), mint_secret());
    }

    #[test]
    fn hash_api_key_is_stable_and_hex() {
        let secret = mint_secret();
        let hash = hash_api_key(&secret);
        assert_eq!(hash, hash_api_key(&secret));
        assert_eq!(hash.len(), 64);
        assert_ne!(hash, hash_api_key(&mint_secret()));
    }

    #[test]
    fn hash_api_key_matches_the_module_vector() {
        // Keep in lockstep with `reducers::api_keys` `hash_api_key_matches_the_gateway_vector`.
        let secret = format!("{SECRET_PREFIX}{}", "ab".repeat(32));
        assert_eq!(
            hash_api_key(&secret),
            "7e264f0416dc383541f0ac3088053aa9f77edcc65371e181fc2a735aa916b7c4"
        );
    }

    #[test]
    fn created_dto_field_is_key_not_key_hash() {
        let names: Vec<_> = std::any::type_name::<CreatedApiKey>().split("::").collect();
        assert!(names.last().is_some_and(|n| *n == "CreatedApiKey"));
        let json = serde_json::to_value(CreatedApiKey {
            id: Uuid::nil(),
            name: "bot".into(),
            prefix: "eiv_aaaaaaaa".into(),
            key: "eiv_aaaaaaaa".into(),
            created_at: 0,
        })
        .unwrap();
        assert!(json.get("key").is_some());
        assert!(json.get("key_hash").is_none());
        assert!(json.get("secret").is_none());
    }
}
