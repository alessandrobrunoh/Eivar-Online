//! HTTP gateway for `apps/frontend` (Angular) and any non-Bevy client.
//!
//! It is a thin facade: the authoritative state lives in SpacetimeDB, and
//! the Bevy desktop client in `bins/game` talks to it directly over its own
//! protocol. This service exists for clients that prefer a plain HTTP surface
//! (login proxy, REST shims, webhooks, ...).
//!
//! Free of gameplay rules — anything that runs authoritatively on the server
//! belongs in `bevymmo_domain` — but *not* stateless: `/auth/*` holds one
//! live SpacetimeDB connection per logged-in browser (see [`stdb::session`]
//! for why a one-shot HTTP call to SpacetimeDB cannot substitute for one),
//! and `/public/*` shares one anonymous connection through
//! [`stdb::directory::PlayerDirectory`].
//!
//! The HTTP surface itself lives in [`api`], one module per area.

mod api;
mod stdb;

use std::net::SocketAddr;
use std::sync::Arc;

use axum::http::{header, HeaderValue, Method};
use tokio::signal;
use tower_http::cors::{AllowOrigin, CorsLayer};
use tracing::info;

use bevymmo_app_support::settings::Settings;
use stdb::directory::PlayerDirectory;
use stdb::session::SessionStore;

#[derive(Clone)]
struct AppState {
    /// SpacetimeDB module the gateway is configured to talk to. Surfaced on
    /// `/` so an operator can confirm wiring without grepping the config.
    spacetime_module: String,
    /// WebSocket URL of the SpacetimeDB instance — see `stdb::connection`
    /// for why the gateway needs a real connection, not a one-shot HTTP call.
    spacetime_uri: String,
    /// Live SpacetimeDB connections, one per authenticated web session.
    sessions: SessionStore,
    /// The one shared connection behind `/public/*`. Connects lazily, on the
    /// first request that needs it, so a missing SpacetimeDB at boot does not
    /// take the whole gateway down. In an `Arc` because the store inside it
    /// is a mutex, not clonable — cloning the handle, not the store.
    directory: Arc<PlayerDirectory>,
    /// Whether the session cookie is marked `Secure`. See
    /// `GatewaySettings::cookie_secure`'s doc comment.
    cookie_secure: bool,
    /// Compiled game catalog (`bevymmo_content`), served on `/v1/public/catalog/*`.
    catalog: Arc<bevymmo_content::catalog::Catalog>,
}

#[tokio::main]
async fn main() {
    let settings = Settings::load();
    init_tracing(&settings.gateway.log_format);

    let bind_addr: SocketAddr = settings
        .gateway
        .bind_addr
        .parse()
        .expect("gateway.bind_addr is not a valid host:port");

    let sessions = SessionStore::new();
    sessions.spawn_reaper();

    let cors_origin_log = settings.gateway.cors_origin.clone();
    let cors_origin: HeaderValue = settings
        .gateway
        .cors_origin
        .parse()
        .expect("gateway.cors_origin is not a valid header value");

    let directory = Arc::new(PlayerDirectory::new(
        settings.spacetime_uri.clone(),
        settings.spacetime_module.clone(),
    ));
    let state = AppState {
        spacetime_module: settings.spacetime_module,
        spacetime_uri: settings.spacetime_uri,
        sessions,
        directory,
        cookie_secure: settings.gateway.cookie_secure,
        catalog: Arc::new(bevymmo_content::catalog::snapshot()),
    };
    let app = api::router(state).layer(cors_layer(cors_origin));

    let listener = tokio::net::TcpListener::bind(bind_addr)
        .await
        .unwrap_or_else(|err| panic!("failed to bind gateway on {bind_addr}: {err}"));

    // Logged because a rejected browser request shows up as an opaque CORS
    // failure in the console with nothing on the server side to match it to.
    info!(
        %bind_addr,
        cors_origin = %cors_origin_log,
        loopback_origins_allowed = cfg!(debug_assertions),
        "BevyMMO gateway listening"
    );

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .expect("gateway server crashed");
}

// `Any` origin is not an option: the session cookie needs
// `Access-Control-Allow-Credentials`, which browsers refuse to honor
// together with a wildcard `Access-Control-Allow-Origin`. See
// `GatewaySettings::cors_origin`'s doc comment.
fn cors_layer(cors_origin: HeaderValue) -> CorsLayer {
    CorsLayer::new()
        // Angular may move to a free port when 4200 is already occupied, so a
        // debug build also accepts any loopback port. A release build accepts
        // the configured origin and nothing else — see `is_local_dev_origin`.
        .allow_origin(AllowOrigin::predicate(move |origin, _| {
            origin == cors_origin || is_local_dev_origin(origin)
        }))
        .allow_credentials(true)
        .allow_methods([Method::GET, Method::POST, Method::DELETE])
        .allow_headers([header::CONTENT_TYPE, header::AUTHORIZATION])
}

/// Whether `origin` is a loopback dev server, and whether that is allowed to
/// matter at all.
///
/// The loopback clause is gated on `debug_assertions` rather than being live
/// everywhere. Paired with `allow_credentials(true)`, an ungated version means
/// *any* page served from any port on a visitor's own machine — another dev
/// server, a desktop app with a local web UI, a tool with a `127.0.0.1` panel —
/// can make credentialed requests to the production gateway and read the
/// replies. `apps/gateway/Dockerfile` builds with `--release`, so the deployed
/// binary drops the clause; `cargo run` keeps it.
fn is_local_dev_origin(origin: &HeaderValue) -> bool {
    if !cfg!(debug_assertions) {
        return false;
    }

    let Ok(origin) = origin.to_str() else {
        return false;
    };

    ["http://localhost:", "http://127.0.0.1:"]
        .iter()
        .any(|prefix| {
            origin
                .strip_prefix(prefix)
                .is_some_and(|port| !port.is_empty() && port.parse::<u16>().is_ok())
        })
}

/// Resolves on the first Ctrl+C / SIGTERM. `axum::serve` then drains in-flight
/// requests before returning, so a deploy does not sever a request mid-flight.
async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => info!("Ctrl+C received, shutting down"),
        _ = terminate => info!("SIGTERM received, shutting down"),
    }
}

fn init_tracing(log_format: &str) {
    use tracing_subscriber::{fmt, EnvFilter};

    let filter = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new("info,bevymmo_gateway=debug"))
        .expect("failed to build log filter");

    // `json`: one object per line for a collector (Loki/ELK/Datadog); the
    // default `text` stays readable in a terminal. Unknown values fall back
    // to text — a typo in the format must not stop the gateway from booting.
    let format = fmt().with_env_filter(filter).with_target(false);
    if log_format.eq_ignore_ascii_case("json") {
        format.json().init();
    } else {
        format.init();
    }
}
