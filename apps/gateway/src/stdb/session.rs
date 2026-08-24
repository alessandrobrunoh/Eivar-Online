//! In-memory store mapping a gateway session cookie to its live
//! [`GatewayConnection`].
//!
//! The cookie value is an opaque id the gateway mints itself — never a
//! SpacetimeDB Identity or token, both of which stay server-side inside the
//! connection this id looks up. A leaked cookie only grants whatever this
//! particular session already had; it grants no direct SpacetimeDB access.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::RwLock;
use uuid::Uuid;

use super::connection::GatewayConnection;

/// How long an idle web session's SpacetimeDB connection stays open.
/// Comfortably longer than a normal browsing session, short enough that a
/// forgotten browser tab does not hold a connection open forever.
const SESSION_IDLE_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// How often the reaper checks for idle sessions. Independent of the
/// timeout itself: this is a polling interval, not a deadline.
const REAP_INTERVAL: Duration = Duration::from_secs(60);

/// Live sessions one account may hold at once.
///
/// Every session is a real WebSocket to SpacetimeDB held for up to
/// [`SESSION_IDLE_TIMEOUT`], so without a cap one account can open as many as
/// it can issue logins for and pin them all for half an hour. The cap is
/// per-account, not global, on purpose: a global cap with eviction would let
/// one noisy account push everybody else's sessions out.
///
/// Sized for a person, not a single tab: a desktop browser, a phone, and a
/// couple of stale tabs that have not been reaped yet all fit.
const MAX_SESSIONS_PER_ACCOUNT: usize = 8;

pub type SessionId = String;

struct Entry {
    connection: GatewayConnection,
    last_seen: Instant,
    /// Cached at insert. `GatewayConnection::account_id` reads it back out of
    /// the replicated `session` table, which is a linear scan and is `None`
    /// once the socket dies — neither of which suits an eviction check.
    account_id: Option<u64>,
}

/// Cloning shares the same underlying map (via `Arc`) — cheap, and what
/// letting every axum handler hold its own copy of the state wants.
#[derive(Clone, Default)]
pub struct SessionStore {
    entries: Arc<RwLock<HashMap<SessionId, Entry>>>,
}

impl SessionStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Mints a new session id for an already-open, already-authenticated
    /// `connection`. Deliberately takes a connection rather than opening one
    /// itself: `register`/`login` must succeed *before* a cookie is issued,
    /// so a failed login attempt never hands the browser a session id for a
    /// connection nobody can use.
    pub async fn create(&self, connection: GatewayConnection) -> SessionId {
        let id = Uuid::new_v4().to_string();
        self.insert(id.clone(), connection).await;
        id
    }

    /// Stores `connection` under a caller-chosen `id`. Cookie sessions use a
    /// random UUID from [`Self::create`]; API-key sessions reuse this with a
    /// stable `ak:{sha256}` id so later Bearer requests reuse the socket.
    pub async fn insert(&self, id: SessionId, connection: GatewayConnection) {
        let account_id = connection.account_id();
        let evicted = {
            let mut entries = self.entries.write().await;
            entries.insert(
                id.clone(),
                Entry {
                    connection,
                    last_seen: Instant::now(),
                    account_id,
                },
            );
            let live: Vec<(SessionId, Option<u64>, Instant)> = entries
                .iter()
                .map(|(id, entry)| (id.clone(), entry.account_id, entry.last_seen))
                .collect();
            over_cap(&live, account_id, &id)
        };

        // Ended outside the lock: `end` takes the same write guard, and
        // `logout` on the evicted connection is a network round-trip.
        for id in evicted {
            tracing::info!(session = %id, "evicting the account's oldest session");
            self.end(&id).await;
        }
    }



    /// The connection for `id`, refreshing its idle timer. `None` if `id` is
    /// unknown or was already reaped for inactivity.
    pub async fn get(&self, id: &str) -> Option<GatewayConnection> {
        let mut entries = self.entries.write().await;
        let entry = entries.get_mut(id)?;
        entry.last_seen = Instant::now();
        Some(entry.connection.clone())
    }

    /// Ends the session: best-effort `logout` on its connection (clears the
    /// server-side `Session` row explicitly rather than waiting for the
    /// socket close to do it), then drops it from the store and disconnects.
    pub async fn end(&self, id: &str) {
        if let Some(entry) = self.entries.write().await.remove(id) {
            let _ = entry.connection.logout().await;
            entry.connection.disconnect();
        }
    }

    /// Closes and drops every connection idle longer than
    /// [`SESSION_IDLE_TIMEOUT`].
    async fn reap_idle(&self) {
        let expired: Vec<SessionId> = {
            let entries = self.entries.read().await;
            entries
                .iter()
                .filter(|(_, entry)| entry.last_seen.elapsed() > SESSION_IDLE_TIMEOUT)
                .map(|(id, _)| id.clone())
                .collect()
        };
        for id in expired {
            tracing::debug!(session = %id, "gateway session idle timeout");
            self.end(&id).await;
        }
    }

    /// Spawns the periodic idle-session reaper. Call once, from `main`.
    pub fn spawn_reaper(&self) {
        let store = self.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(REAP_INTERVAL);
            loop {
                interval.tick().await;
                store.reap_idle().await;
            }
        });
    }
}

/// Ids to evict so `account_id` is back within [`MAX_SESSIONS_PER_ACCOUNT`],
/// oldest first, never including `keep`.
///
/// Anonymous sessions (`account_id` of `None`) are not capped: those are the
/// `/public/*` directory handle and pre-authentication connections, not
/// something a caller accumulates by logging in.
///
/// A free function over plain tuples rather than a method over the map, so the
/// eviction arithmetic — the part where an off-by-one either evicts a session
/// that should have lived or lets the cap drift upward — is testable without a
/// live SpacetimeDB connection to put in an `Entry`.
fn over_cap(
    live: &[(SessionId, Option<u64>, Instant)],
    account_id: Option<u64>,
    keep: &str,
) -> Vec<SessionId> {
    let Some(account_id) = account_id else {
        return Vec::new();
    };

    let mut owned: Vec<(SessionId, Instant)> = live
        .iter()
        .filter(|(id, owner, _)| *owner == Some(account_id) && id.as_str() != keep)
        .map(|(id, _, last_seen)| (id.clone(), *last_seen))
        .collect();

    // `keep` is excluded above but still occupies one slot of the allowance.
    let allowance = MAX_SESSIONS_PER_ACCOUNT.saturating_sub(1);
    if owned.len() <= allowance {
        return Vec::new();
    }

    owned.sort_by_key(|(_, last_seen)| *last_seen);
    owned.truncate(owned.len() - allowance);
    owned.into_iter().map(|(id, _)| id).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(id: &str, account: Option<u64>, age_secs: u64) -> (SessionId, Option<u64>, Instant) {
        let base = Instant::now();
        (
            id.to_string(),
            account,
            base - Duration::from_secs(age_secs),
        )
    }

    #[test]
    fn an_account_under_the_cap_evicts_nothing() {
        let live: Vec<_> = (0..MAX_SESSIONS_PER_ACCOUNT - 1)
            .map(|n| session(&format!("s{n}"), Some(1), n as u64))
            .collect();
        assert!(over_cap(&live, Some(1), "s0").is_empty());
    }

    #[test]
    fn the_oldest_session_goes_when_the_cap_is_exceeded() {
        // `new` is the one just inserted; the rest are older, `old` oldest.
        let live = vec![
            session("new", Some(1), 0),
            session("old", Some(1), 900),
            session("mid", Some(1), 60),
        ];
        // Cap of 8 leaves room for all three, so nothing goes yet.
        assert!(over_cap(&live, Some(1), "new").is_empty());

        let mut many: Vec<_> = (0..MAX_SESSIONS_PER_ACCOUNT)
            .map(|n| session(&format!("s{n}"), Some(1), (n as u64 + 1) * 10))
            .collect();
        many.push(session("new", Some(1), 0));
        let evicted = over_cap(&many, Some(1), "new");
        assert_eq!(evicted.len(), 1);
        // Highest age_secs is the oldest.
        assert_eq!(evicted[0], format!("s{}", MAX_SESSIONS_PER_ACCOUNT - 1));
    }

    #[test]
    fn the_session_being_inserted_is_never_evicted() {
        let mut live: Vec<_> = (0..MAX_SESSIONS_PER_ACCOUNT * 2)
            .map(|n| session(&format!("s{n}"), Some(1), 10))
            .collect();
        live.push(session("new", Some(1), 0));
        let evicted = over_cap(&live, Some(1), "new");
        assert!(!evicted.iter().any(|id| id == "new"));
    }

    #[test]
    fn one_account_cannot_evict_another() {
        let mut live: Vec<_> = (0..MAX_SESSIONS_PER_ACCOUNT * 3)
            .map(|n| session(&format!("attacker{n}"), Some(1), n as u64))
            .collect();
        live.push(session("victim", Some(2), 9_999));
        live.push(session("new", Some(1), 0));

        let evicted = over_cap(&live, Some(1), "new");
        assert!(!evicted.is_empty(), "the noisy account is trimmed");
        assert!(
            !evicted.iter().any(|id| id == "victim"),
            "another account's session must survive however old it is"
        );
    }

    #[test]
    fn anonymous_sessions_are_not_capped() {
        let live: Vec<_> = (0..MAX_SESSIONS_PER_ACCOUNT * 5)
            .map(|n| session(&format!("anon{n}"), None, n as u64))
            .collect();
        assert!(over_cap(&live, None, "anon0").is_empty());
    }

    #[test]
    fn the_account_lands_exactly_on_the_cap() {
        let mut live: Vec<_> = (0..MAX_SESSIONS_PER_ACCOUNT * 2)
            .map(|n| session(&format!("s{n}"), Some(1), n as u64 + 1))
            .collect();
        live.push(session("new", Some(1), 0));
        let evicted = over_cap(&live, Some(1), "new");
        let survivors = live.len() - evicted.len();
        assert_eq!(survivors, MAX_SESSIONS_PER_ACCOUNT);
    }
}
