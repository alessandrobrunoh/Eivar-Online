//! Authenticated character-scoped reads. Cookie session or Bearer API key.

use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::routing::get;
use axum::Json;
use axum::Router;
use serde::Serialize;
use uuid::Uuid;

use crate::api::api_keys::owned_character;
use crate::api::auth::resolve_connection;
use crate::api::error::AppError;
use crate::AppState;

#[derive(Serialize, utoipa::ToSchema)]
pub struct WalletResponse {
    pub gold: u64,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct StatsResponse {
    pub current_health: f32,
    pub max_health: f32,
    pub current_shield: f32,
    pub max_shield: f32,
    pub max_mana: f32,
    pub mana_regeneration: f32,
    pub armor: f32,
    pub movement_speed: f32,
    pub attack_power: f32,
    pub gathering_speed: f32,
    pub gathering_bonus: f32,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/characters/:character_id/wallet", get(wallet))
        .route("/v1/characters/:character_id/stats", get(stats))
}

/// Gold on one of the caller's characters. 0 if the character exists but
/// has no `character_wallet` row yet.
#[utoipa::path(
    get,
    tag = "auth",
    path = "/v1/characters/{character_id}/wallet",
    params(("character_id" = Uuid, Path, description = "Character UUID from /v1/profile")),
    responses(
        (status = 200, description = "Wallet gold for this character", body = WalletResponse),
        (status = 401, description = "No session, or the session expired", body = crate::api::error::ErrorResponse),
        (status = 403, description = "The character belongs to another account", body = crate::api::error::ErrorResponse),
        (status = 404, description = "No character with that id", body = crate::api::error::ErrorResponse),
    ),
)]
pub async fn wallet(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(character_id): Path<Uuid>,
) -> Result<Json<WalletResponse>, AppError> {
    let connection = resolve_connection(&state, &headers, true).await?;
    owned_character(&connection, character_id)?;
    Ok(Json(WalletResponse {
        gold: connection.wallet_gold(character_id),
    }))
}

/// Base stats of one of the caller's characters (`player_stats`, no equipment).
#[utoipa::path(
    get,
    tag = "auth",
    path = "/v1/characters/{character_id}/stats",
    params(("character_id" = Uuid, Path, description = "Character UUID from /v1/profile")),
    responses(
        (status = 200, description = "Base stats for this character", body = StatsResponse),
        (status = 401, description = "No session, or the session expired", body = crate::api::error::ErrorResponse),
        (status = 403, description = "The character belongs to another account", body = crate::api::error::ErrorResponse),
        (status = 404, description = "No character with that id", body = crate::api::error::ErrorResponse),
    ),
)]
pub async fn stats(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(character_id): Path<Uuid>,
) -> Result<Json<StatsResponse>, AppError> {
    let connection = resolve_connection(&state, &headers, true).await?;
    owned_character(&connection, character_id)?;
    let Some(stats) = connection.player_stats(character_id) else {
        return Err(AppError::NotFound(format!(
            "no stats for character {character_id}"
        )));
    };
    Ok(Json(StatsResponse {
        current_health: stats.current_health,
        max_health: stats.max_health,
        current_shield: stats.current_shield,
        max_shield: stats.max_shield,
        max_mana: stats.max_mana,
        mana_regeneration: stats.mana_regeneration,
        armor: stats.armor,
        movement_speed: stats.movement_speed,
        attack_power: stats.attack_power,
        gathering_speed: stats.gathering_speed,
        gathering_bonus: stats.gathering_bonus,
    }))
}
