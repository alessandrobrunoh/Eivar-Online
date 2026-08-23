//! One live SpacetimeDB connection, wrapped for use from axum handlers.
//!
//! # Why a real connection, not a one-shot HTTP call
//!
//! SpacetimeDB's `POST /v1/database/<name>/call/<reducer>` looks like it
//! could authenticate a web session statelessly: pass a bearer token, get a
//! result. It cannot, for this module specifically. Verified empirically
//! against the local server: SpacetimeDB treats *every* HTTP call as its own
//! connection — `client_connected` fires before the reducer runs and
//! `client_disconnected` fires immediately after, even when the same
//! Identity token is reused across calls. `client_disconnected`
//! (`reducers::lifecycle::client_disconnected`) unconditionally deletes the
//! caller's `Session` row, so a `Session` created by `register`/`login` over
//! HTTP is already gone before the HTTP response comes back — reusing the
//! same token does not help, because it is the *connection*, not the
//! Identity, that `Session`'s lifetime is tied to.
//!
//! So a web session needs the same thing a game session has: a connection
//! that stays open. This type holds one, driven by a background task calling
//! [`spacetimedb_sdk`]'s `run_async` for as long as the session lives — see
//! `super::session` for how long that is and how it is torn down.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use spacetimedb_sdk::{DbContext, Table};
use tokio::sync::oneshot;

use super::module_bindings::api_key_meta_type::ApiKeyMeta;
use super::module_bindings::authenticate_api_key_reducer::authenticate_api_key;
use super::module_bindings::create_api_key_reducer::create_api_key;
use super::module_bindings::login_reducer::login;
use super::module_bindings::logout_reducer::logout;
use super::module_bindings::register_reducer::register;
use super::module_bindings::revoke_api_key_reducer::revoke_api_key;
use super::module_bindings::stats_row_type::StatsRow;
use super::module_bindings::{
    CharacterWalletTableAccess, DbConnection, EntityStatsTableAccess, ErrorContext, Market, MarketBuyOrder,
    MarketBuyOrderTableAccess, MarketSellOrder, MarketSellOrderTableAccess, MarketTableAccess,
    MyApiKeysTableAccess, Player, PlayerTableAccess, ReducerEventContext,
    SessionTableAccess,
};

/// One of the caller's own characters, for the `/profile` endpoint.
#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
pub struct CharacterSummary {
    /// Serialized as a hyphenated UUID string in JSON.
    pub character_id: uuid::Uuid,
    pub display_name: String,
    pub online: bool,
}

/// What a `*_then` callback is handed: the module's own `Result`, or the SDK
/// failing to decode one. Mirrors `bevymmo_client::stdb::plugin`'s type of
/// the same name — same SDK, same shape.
type ReducerOutcome = Result<Result<(), String>, spacetimedb_sdk::__codegen::InternalError>;

/// Turns a reducer's `_then` callback into something `await`-able: sends the
/// outcome down `tx` instead of requiring the caller to register their own
/// callback and poll for it.
fn outcome_sender(
    tx: oneshot::Sender<Result<(), String>>,
) -> impl FnOnce(&ReducerEventContext, ReducerOutcome) + Send + 'static {
    move |_ctx, outcome| {
        let result = match outcome {
            Ok(Ok(())) => Ok(()),
            Ok(Err(reason)) => Err(reason),
            Err(err) => Err(err.to_string()),
        };
        // The receiver may already be gone if the HTTP request that started
        // this call timed out or the client disconnected; nothing to do.
        let _ = tx.send(result);
    }
}

/// A live SpacetimeDB connection plus the background task advancing it.
/// Cloning shares the same underlying connection — cheap, and what the
/// session store wants: every request for the same browser session reuses
/// one connection rather than opening a new one.
#[derive(Clone)]
pub struct GatewayConnection {
    conn: Arc<DbConnection>,
    /// Set by the background task once `run_async` returns, i.e. the socket is
    /// gone. Read by [`Self::is_closed`] so long-lived holders (the public
    /// directory) can tell a live cache from a dead one and reconnect.
    closed: Arc<AtomicBool>,
}

impl GatewayConnection {
    /// Opens a fresh connection, subscribes to the tables this gateway
    /// needs, and spawns the background task that keeps it alive. Returns
    /// once the initial subscription has applied, so callers can read
    /// `player`/`session` state immediately after this returns.
    pub async fn connect(uri: &str, module: &str) -> Result<Self, String> {
        let conn = DbConnection::builder()
            .with_uri(uri)
            .with_database_name(module)
            .on_connect_error(|_ctx: &ErrorContext, err| {
                tracing::error!("gateway SpacetimeDB connection error: {err}");
            })
            .on_disconnect(|_ctx, err| match err {
                Some(err) => tracing::warn!("gateway SpacetimeDB connection dropped: {err}"),
                None => tracing::debug!("gateway SpacetimeDB connection closed"),
            })
            .build()
            .map_err(|err| err.to_string())?;
        let conn = Arc::new(conn);

        let closed = Arc::new(AtomicBool::new(false));
        let run_conn = Arc::clone(&conn);
        let closed_flag = Arc::clone(&closed);
        tokio::spawn(async move {
            if let Err(err) = run_conn.run_async().await {
                tracing::warn!("gateway SpacetimeDB connection ended: {err}");
            }
            closed_flag.store(true, Ordering::Release);
        });

        let this = Self { conn, closed };
        this.subscribe().await?;
        Ok(this)
    }

    /// Subscribes to the public tables this gateway reads, then awaits the
    /// initial snapshot so callers do not race an empty local cache.
    ///
    /// `player` / `session` back the character roster and this connection's
    /// `account_id`. `market` / `market_sell_order` / `market_buy_order` /
    /// `character_wallet` back the public market APIs and the wallet lookup.
    /// `my_api_keys` is the per-caller view of API-key metadata (no hashes).
    /// `player_stats` backs `/v1/characters/:id/stats`.
    async fn subscribe(&self) -> Result<(), String> {
        let (tx, rx) = oneshot::channel();
        let tx = Arc::new(Mutex::new(Some(tx)));

        let applied_tx = Arc::clone(&tx);
        let error_tx = Arc::clone(&tx);
        self.conn
            .subscription_builder()
            .on_applied(move |_ctx| {
                if let Some(tx) = applied_tx.lock().unwrap().take() {
                    let _ = tx.send(Ok(()));
                }
            })
            .on_error(move |_ctx, err| {
                if let Some(tx) = error_tx.lock().unwrap().take() {
                    let _ = tx.send(Err(err.to_string()));
                }
            })
            .subscribe([
                "SELECT * FROM player",
                "SELECT * FROM session",
                "SELECT * FROM market",
                "SELECT * FROM market_sell_order",
                "SELECT * FROM market_buy_order",
                "SELECT * FROM character_wallet",
                "SELECT * FROM my_api_keys",
                "SELECT * FROM player_stats",
            ]);

        rx.await
            .map_err(|_| "subscription dropped before it applied".to_string())?
    }

    /// Whether the underlying socket has gone away. Deliberately a flag set
    /// when `run_async` exits rather than a live check: between the socket
    /// dropping and the runtime task observing it, readers may briefly see
    /// stale cache contents — acceptable for a directory that only serves
    /// already-public rows, and self-heals on the next reconnect.
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    /// This connection's own SpacetimeDB `Identity`, once the handshake has
    /// completed (always true by the time [`Self::connect`] returns).
    fn identity(&self) -> Option<spacetimedb_sdk::Identity> {
        self.conn.try_identity()
    }

    /// The `account_id` this connection authenticated as, if `register`/
    /// `login` has succeeded on it. Resolved from the (public) `session`
    /// table rather than tracked locally, so it is always consistent with
    /// what the server actually recorded.
    pub fn account_id(&self) -> Option<u64> {
        let identity = self.identity()?;
        self.conn
            .db()
            .session()
            .iter()
            .find(|row| row.identity == identity)
            .map(|row| row.account_id)
    }

    /// This account's own characters (up to
    /// `bevymmo_module::MAX_CHARACTERS_PER_ACCOUNT`), from the already
    /// public `player` table filtered to this connection's `account_id`.
    /// `None` if this connection has not authenticated yet.
    pub fn characters(&self) -> Option<Vec<CharacterSummary>> {
        let account_id = self.account_id()?;
        Some(
            self.conn
                .db()
                .player()
                .iter()
                .filter(|row| row.account_id == account_id)
                .map(character_summary)
                .collect(),
        )
    }

    /// Every `player` row in this connection's local cache — public data, kept
    /// live by the subscription in [`Self::subscribe`]. The backing store for
    /// the `/public/accounts/*` endpoints; see `super::directory`.
    ///
    /// Collected rather than returned as an iterator: `db()` hands out an
    /// owned handle whose borrow cannot escape this method. The table is one
    /// row per character ever created, so the copy is not worth fighting for.
    pub fn players(&self) -> Vec<Player> {
        self.conn.db().player().iter().collect()
    }

    /// Every `market` row in this connection's local cache.
    pub fn markets(&self) -> Vec<Market> {
        self.conn.db().market().iter().collect()
    }

    /// Every `market_sell_order` row in this connection's local cache.
    pub fn sell_orders(&self) -> Vec<MarketSellOrder> {
        self.conn.db().market_sell_order().iter().collect()
    }

    /// Every `market_buy_order` row in this connection's local cache.
    pub fn buy_orders(&self) -> Vec<MarketBuyOrder> {
        self.conn.db().market_buy_order().iter().collect()
    }

    /// One `player` row by character UUID, if present in the local cache.
    pub fn player(&self, character_id: uuid::Uuid) -> Option<Player> {
        let character_id = spacetimedb_sdk::Uuid::from_u128(character_id.as_u128());
        self.conn
            .db()
            .player()
            .iter()
            .find(|row| row.character_id == character_id)
    }

    /// Gold on `character_id`'s wallet, or `0` if no wallet row exists yet.
    pub fn wallet_gold(&self, character_id: uuid::Uuid) -> u64 {
        let character_id = spacetimedb_sdk::Uuid::from_u128(character_id.as_u128());
        self.conn
            .db()
            .character_wallet()
            .iter()
            .find(|row| row.character_id == character_id)
            .map(|row| row.gold)
            .unwrap_or(0)
    }

    pub async fn register(&self, email: String, password: String) -> Result<(), String> {
        let (tx, rx) = oneshot::channel();
        self.conn
            .reducers()
            .register_then(email, password, outcome_sender(tx))
            .map_err(|err| err.to_string())?;
        rx.await
            .map_err(|_| "connection closed before the server replied".to_string())?
    }

    pub async fn login(&self, email: String, password: String) -> Result<(), String> {
        let (tx, rx) = oneshot::channel();
        self.conn
            .reducers()
            .login_then(email, password, outcome_sender(tx))
            .map_err(|err| err.to_string())?;
        rx.await
            .map_err(|_| "connection closed before the server replied".to_string())?
    }

    /// The caller's API-key metadata from the `my_api_keys` view. Empty until
    /// this connection has authenticated (cookie login or `authenticate_api_key`).
    pub fn api_keys(&self) -> Vec<ApiKeyMeta> {
        self.conn.db().my_api_keys().iter().collect()
    }

    pub fn api_key(&self, id: uuid::Uuid) -> Option<ApiKeyMeta> {
        let id = spacetimedb_sdk::Uuid::from_u128(id.as_u128());
        self.conn.db().my_api_keys().iter().find(|row| row.id == id)
    }

    /// Effective stats for one character, including current health and shield.
    ///
    /// `player_stats` stores base values, while `entity_stats` is the
    /// authoritative runtime row used by combat and replication.
    pub fn player_stats(&self, character_id: uuid::Uuid) -> Option<StatsRow> {
        let player = self.player(character_id)?;
        self.conn
            .db()
            .entity_stats()
            .entity_id()
            .find(&player.entity_id)
            .map(|row| row.stats)
    }

    pub async fn create_api_key(
        &self,
        id: uuid::Uuid,
        name: String,
        prefix: String,
        secret: String,
    ) -> Result<(), String> {
        let (tx, rx) = oneshot::channel();
        let id = spacetimedb_sdk::Uuid::from_u128(id.as_u128());
        self.conn
            .reducers()
            .create_api_key_then(id, name, prefix, secret, outcome_sender(tx))
            .map_err(|err| err.to_string())?;
        rx.await
            .map_err(|_| "connection closed before the server replied".to_string())?
    }

    pub async fn revoke_api_key(&self, id: uuid::Uuid) -> Result<(), String> {
        let (tx, rx) = oneshot::channel();
        let id = spacetimedb_sdk::Uuid::from_u128(id.as_u128());
        self.conn
            .reducers()
            .revoke_api_key_then(id, outcome_sender(tx))
            .map_err(|err| err.to_string())?;
        rx.await
            .map_err(|_| "connection closed before the server replied".to_string())?
    }

    pub async fn authenticate_api_key(&self, secret: String) -> Result<(), String> {
        let (tx, rx) = oneshot::channel();
        self.conn
            .reducers()
            .authenticate_api_key_then(secret, outcome_sender(tx))
            .map_err(|err| err.to_string())?;
        rx.await
            .map_err(|_| "connection closed before the server replied".to_string())?
    }

    /// Ends this connection's authenticated session server-side (deletes its
    /// `Session` row) without closing the socket. A no-op if it was never
    /// authenticated. The gateway calls this and then drops the connection
    /// entirely — see `super::session::SessionStore::end`.
    pub async fn logout(&self) -> Result<(), String> {
        let (tx, rx) = oneshot::channel();
        self.conn
            .reducers()
            .logout_then(outcome_sender(tx))
            .map_err(|err| err.to_string())?;
        rx.await
            .map_err(|_| "connection closed before the server replied".to_string())?
    }

    /// Closes the underlying socket. `client_disconnected` on the module
    /// then deletes this connection's `Session` row server-side, same as an
    /// explicit [`Self::logout`] — see `reducers::lifecycle::client_disconnected`.
    pub fn disconnect(&self) {
        if let Err(err) = self.conn.disconnect() {
            tracing::warn!("failed to queue gateway SpacetimeDB disconnect: {err}");
        }
    }
}

fn character_summary(row: Player) -> CharacterSummary {
    CharacterSummary {
        character_id: uuid::Uuid::from_u128(row.character_id.as_u128()),
        display_name: row.display_name,
        online: row.online,
    }
}
