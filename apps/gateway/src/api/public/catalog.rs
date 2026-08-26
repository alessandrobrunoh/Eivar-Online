//! `/public/catalog/*` — compiled game content, readable without a session.
//!
//! Unlike markets and accounts, this is **not** SpacetimeDB state. The
//! gateway builds the snapshot from `bevymmo_content` at process start and
//! serves it from memory, so these routes stay 200 when the module is down.

use axum::extract::{Path, State};
use axum::routing::get;
use axum::Json;
use axum::Router;
use bevymmo_content::catalog::{Catalog, CatalogItem};

use crate::api::error::AppError;
use crate::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/public/catalog", get(get_catalog))
        .route("/v1/public/catalog/items", get(list_items))
        .route("/v1/public/catalog/items/:id", get(get_item))
}

fn find_item<'a>(catalog: &'a Catalog, id: &str) -> Result<&'a CatalogItem, AppError> {
    catalog
        .item(id)
        .ok_or_else(|| AppError::NotFound(format!("unknown catalog item `{id}`")))
}

/// Full compiled catalog. Later slices add collections alongside `items`.
#[utoipa::path(
    get,
    tag = "catalog",
    path = "/v1/public/catalog",
    responses((status = 200, description = "Compiled game catalog", body = Catalog)),
)]
pub async fn get_catalog(State(state): State<AppState>) -> Json<Catalog> {
    Json((*state.catalog).clone())
}

/// Every item shipped by this game build, sorted by id.
#[utoipa::path(
    get,
    tag = "catalog",
    path = "/v1/public/catalog/items",
    responses((status = 200, description = "Compiled items", body = Vec<CatalogItem>)),
)]
pub async fn list_items(State(state): State<AppState>) -> Json<Vec<CatalogItem>> {
    Json(state.catalog.items.clone())
}

/// One item by stable id (`sword`, `wood`, …).
#[utoipa::path(
    get,
    tag = "catalog",
    path = "/v1/public/catalog/items/{id}",
    params(("id" = String, Path, description = "Stable item id (e.g. sword)")),
    responses(
        (status = 200, description = "The item", body = CatalogItem),
        (status = 404, description = "No item with that id", body = crate::api::error::ErrorResponse),
    ),
)]
pub async fn get_item(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<CatalogItem>, AppError> {
    find_item(&state.catalog, &id).cloned().map(Json)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stdb::directory::PlayerDirectory;
    use crate::stdb::session::SessionStore;
    use std::sync::Arc;

    fn test_state() -> AppState {
        AppState {
            spacetime_module: "test".into(),
            spacetime_uri: "http://127.0.0.1:9".into(),
            sessions: SessionStore::new(),
            directory: Arc::new(PlayerDirectory::new("http://127.0.0.1:9", "test")),
            cookie_secure: false,
            catalog: Arc::new(bevymmo_content::catalog::snapshot()),
            auth_limiter: crate::api::rate_limit::RateLimiter::new(),
            trust_proxy_headers: false,
        }
    }

    #[tokio::test]
    async fn list_items_returns_the_compiled_catalog_without_stdb() {
        let Json(items) = list_items(State(test_state())).await;
        assert!(
            items.iter().any(|item| item.id == "sword"),
            "compiled catalog must include the sword"
        );
        assert!(items.windows(2).all(|pair| pair[0].id <= pair[1].id));
    }

    #[tokio::test]
    async fn get_catalog_is_the_same_snapshot() {
        let Json(catalog) = get_catalog(State(test_state())).await;
        assert_eq!(catalog, bevymmo_content::catalog::snapshot());
    }

    #[tokio::test]
    async fn get_item_returns_the_sword() {
        let Json(item) = get_item(State(test_state()), Path("sword".into()))
            .await
            .expect("sword is in the catalog");
        assert_eq!(item.name, "Spada");
        assert_eq!(item.category, "Weapon");
        let crafting = item.crafting.expect("sword has a recipe");
        assert!((crafting.channel_seconds - 3.0).abs() < f32::EPSILON);
        assert_eq!(crafting.ingredients.len(), 2);
        assert_eq!(crafting.ingredients[0].id, "wood");
        assert_eq!(crafting.ingredients[0].amount, 2);
        assert_eq!(crafting.ingredients[1].id, "copper");
        assert_eq!(crafting.ingredients[1].amount, 4);
    }

    #[tokio::test]
    async fn unknown_item_is_not_found() {
        let err = get_item(State(test_state()), Path("channeling-staff".into()))
            .await
            .expect_err("prototype wiki slug is not a catalog id");
        match err {
            AppError::NotFound(message) => {
                assert!(message.contains("channeling-staff"));
            }
            other => panic!("expected NotFound, got {other}"),
        }
    }

    #[test]
    fn find_item_does_not_need_a_live_module() {
        let catalog = bevymmo_content::catalog::snapshot();
        assert!(find_item(&catalog, "wood").is_ok());
        assert!(matches!(
            find_item(&catalog, "nope"),
            Err(AppError::NotFound(_))
        ));
    }
}
