//! Layered configuration: defaults <- `config/default.toml` <-
//! `config/<APP_ENV>.toml` <- `config/local.toml` <- env vars.
//!
//! Secrets should not be committed: use `config/local.toml` (gitignored)
//! or env vars (e.g. `DATABASE_URL`).

use serde::Deserialize;

const DEFAULT_TOML: &str = include_str!("../../../config/default.toml");

/// Application configuration loaded from `config/*.toml` files
/// and env vars.
#[derive(Debug, Clone, Deserialize)]
pub struct Settings {
    /// Filter string for `LogPlugin` matching `RUST_LOG` format.
    #[serde(default = "default_log_filter")]
    pub log_filter: String,

    /// Fixed tick rate (Hz). The `Fixed` schedule uses `1.0 / tick_rate`.
    #[serde(default = "default_tick_rate")]
    pub tick_rate: f64,

    /// WebSocket URL of the SpacetimeDB instance.
    #[serde(default = "default_spacetime_uri")]
    pub spacetime_uri: String,

    /// Name the module is published under (`spacetime publish <name>`).
    #[serde(default = "default_spacetime_module")]
    pub spacetime_module: String,

    #[serde(default)]
    pub gateway: GatewaySettings,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct GatewaySettings {
    /// Local address the HTTP gateway binds to ("host:port").
    ///
    /// Kept as `String` because the `config` crate hands us strings;
    /// conversion happens at call sites.
    #[serde(default = "default_gateway_bind_addr")]
    pub bind_addr: String,

    /// Origin the frontend is served from, for CORS. The gateway's session
    /// cookie requires credentialed cross-origin requests
    /// (`Access-Control-Allow-Credentials`), which browsers refuse to pair
    /// with a wildcard `*` origin — this must be the frontend's exact
    /// scheme+host+port. Override per environment (`config/production.toml`,
    /// `GATEWAY__CORS_ORIGIN`) rather than trusting this default anywhere but
    /// local development.
    #[serde(default = "default_cors_origin")]
    pub cors_origin: String,

    /// Whether the session cookie is marked `Secure` (HTTPS-only). `false`
    /// by default so plain-HTTP local development keeps working — browsers
    /// silently drop `Secure` cookies over HTTP, which would otherwise look
    /// like login "succeeding" but never actually persisting. Production
    /// must set this `true` once served over HTTPS.
    #[serde(default)]
    pub cookie_secure: bool,

    /// Whether `X-Forwarded-For` is allowed to name the client, for rate
    /// limiting. `false` by default, and it must stay `false` unless a reverse
    /// proxy under your control is the only thing that can reach the gateway's
    /// port: the header is caller-supplied, so trusting it on a directly
    /// exposed service lets anyone choose their own rate-limit bucket and pick
    /// a fresh one per request. Set `true` alongside the `caddy` service in
    /// `docker-compose.yml`.
    #[serde(default)]
    pub trust_proxy_headers: bool,

    /// Log output format: `"text"` (default, human-readable) or `"json"`
    /// (one JSON object per line, for a log collector). Production sets
    /// `json` via `GATEWAY__LOG_FORMAT`; an unknown value falls back to
    /// `text` rather than refusing to boot — a wrong format must not take
    /// the service down.
    #[serde(default = "default_gateway_log_format")]
    pub log_format: String,
}

impl Settings {
    /// Loads configuration merging sources in order:
    /// `default.toml` < `config/<APP_ENV>.toml` < `config/local.toml` < env vars.
    ///
    /// `APP_ENV` is read from env var (default: `development`).
    pub fn load() -> Self {
        let env = std::env::var("APP_ENV").unwrap_or_else(|_| "development".to_owned());

        let builder = config::Config::builder()
            // Committed defaults.
            .add_source(config::File::from_str(
                DEFAULT_TOML,
                config::FileFormat::Toml,
            ))
            // Current profile (development/production/...). Not required:
            // if missing, defaults are used.
            .add_source(config::File::with_name(&format!("config/{env}")).required(false))
            // Local gitignored overrides.
            .add_source(config::File::with_name("config/local").required(false))
            // Env vars override everything. `try_parsing(true)` lets
            // values like "1.0" be parsed as numbers where needed.
            // Double underscores map to nested fields, so the gateway's bind
            // address is `BEVYMMO__GATEWAY__BIND_ADDR`.
            //
            // The `BEVYMMO` prefix is not cosmetic. Without it this source
            // ingests *every* environment variable, so any name that collides
            // with a field silently overrides the config — and a bare
            // `GATEWAY` variable, which collides with the whole nested table,
            // makes `try_deserialize` fail and takes the process down at boot.
            .add_source(
                config::Environment::with_prefix("BEVYMMO")
                    .separator("__")
                    .try_parsing(true),
            );

        builder
            .build()
            .expect("failed to build configuration")
            .try_deserialize()
            .expect("failed to deserialize configuration")
    }
}

fn default_log_filter() -> String {
    "warn,bevy_lightyear_game=debug,lightyear=info".to_owned()
}

fn default_tick_rate() -> f64 {
    60.0
}

fn default_spacetime_uri() -> String {
    "ws://127.0.0.1:3000".to_string()
}

fn default_spacetime_module() -> String {
    "bevymmo".to_string()
}

fn default_gateway_bind_addr() -> String {
    "127.0.0.1:8080".to_owned()
}

fn default_cors_origin() -> String {
    "http://localhost:4200".to_owned()
}

fn default_gateway_log_format() -> String {
    "text".to_owned()
}
