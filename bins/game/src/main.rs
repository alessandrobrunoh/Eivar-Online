use bevy::log::LogPlugin;
use bevy::prelude::*;
#[cfg(feature = "client")]
use bevy::window::PresentMode;
use clap::{Parser, Subcommand};
use core::time::Duration;
use std::path::{Path, PathBuf};

use bevymmo_app_support::paths;
use bevymmo_app_support::settings::Settings;
use bevymmo_client::app_state as game_state;
use bevymmo_network::network::mode;

#[derive(Parser, Debug)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    mode: Mode,
}

#[derive(Subcommand, Debug)]
enum Mode {
    /// Run the game client.
    ///
    /// There is no `server` or `host-client` any more: the authoritative server
    /// is the SpacetimeDB module (`docker compose up -d spacetimedb`, then
    /// `./scripts/stdb.sh publish`), not a Bevy process.
    Client {
        /// SpacetimeDB URL override (default: from config).
        #[arg(long)]
        uri: Option<String>,

        /// Published module name override (default: from config).
        #[arg(long)]
        module: Option<String>,
    },
}

struct AppConfig {
    mode: mode::AppMode,
    log_filter: String,
    tick_rate: f64,
    spacetime_uri: String,
    spacetime_module: String,
    assets_root: PathBuf,
}

impl AppConfig {
    /// Merges default values from `config/*.toml` files with CLI overrides.
    fn resolve(mode: Mode, settings: Settings) -> Self {
        let Mode::Client { uri, module } = mode;
        Self {
            mode: mode::AppMode::Client,
            tick_rate: settings.tick_rate,
            log_filter: settings.log_filter.clone(),
            spacetime_uri: uri.unwrap_or(settings.spacetime_uri),
            spacetime_module: module.unwrap_or(settings.spacetime_module),
            assets_root: paths::assets_root(),
        }
    }
}

fn main() -> Result<(), String> {
    let settings = Settings::load();
    let config = AppConfig::resolve(Cli::parse().mode, settings);
    ensure_assets_root_exists(&config.assets_root)?;
    println!("Starting {:?}", config.mode);
    build_app(&config).run();
    Ok(())
}

/// Converts missing client assets into one actionable startup error.
///
/// Bevy otherwise logs one error per missing image or GLB, which hides the real
/// deployment issue: `game.exe` was copied without the sibling `assets/` folder.
/// Keeping this check in the composition root avoids coupling lower-level
/// crates to packaging concerns.
///
/// # Errors
/// Returns a message suitable for CLI output when the resolved assets folder is
/// absent.
///
/// # Example
/// ```rust,ignore
/// let assets_root = bevymmo_app_support::paths::assets_root();
/// ensure_assets_root_exists(&assets_root)?;
/// ```
fn ensure_assets_root_exists(assets_root: &Path) -> Result<(), String> {
    if assets_root.is_dir() {
        return Ok(());
    }

    Err(format!(
        "Could not find the client assets directory at '{}'.\n\
         Run from the repository root, or distribute the client as a folder with \
         'game.exe' next to the 'assets' directory.\n\
         Windows bundle command: powershell -ExecutionPolicy Bypass -File \
         .\\scripts\\package-client.ps1",
        assets_root.display()
    ))
}

fn build_app(config: &AppConfig) -> App {
    let mut app = App::new();
    add_platform_plugins(&mut app, config);
    app.insert_resource(config.mode);
    app.add_plugins(game_state::GameStatePlugin);

    #[cfg(feature = "client")]
    if config.mode.has_client() {
        app.add_plugins(bevymmo_client::stdb::StdbPlugin {
            uri: config.spacetime_uri.clone(),
            module: config.spacetime_module.clone(),
        });
    }

    #[cfg(feature = "client")]
    if config.mode.has_client() {
        app.add_plugins(bevymmo_client::player_movement::PlayerMovementPlugin);
        app.add_plugins(bevymmo_client::targeting::TargetingPlugin);
        app.add_plugins(bevymmo_client::gathering::GatheringPlugin);
        app.add_plugins(bevymmo_client::loot::LootPlugin);
        // Load client-side model/animation asset collections before renderer
        // systems try to spawn player and enemy scenes.
        app.add_plugins(bevymmo_presentation::PresentationCorePlugin);
        app.add_plugins(bevymmo_presentation::PresentationPlugin);
    }

    app
}

fn add_platform_plugins(app: &mut App, config: &AppConfig) {
    if config.mode.is_windowed() {
        #[cfg(feature = "client")]
        {
            // Names the window after the module it is talking to. The old title
            // carried a Netcode client id, which no longer exists: SpacetimeDB
            // identifies a connection by its `Identity`, and the client does not
            // know that until it has connected.
            let title = format!("BevyMMO — {}", config.spacetime_module);
            app.add_plugins(
                DefaultPlugins
                    // The runnable package lives in `bins/game`, while the
                    // workspace keeps shared assets at the repository root.
                    // The assets folder is resolved by walking up from the
                    // executable / working directory so the same binary works
                    // whether launched via `cargo run` or by double-clicking
                    // `target/debug/game.exe`.
                    .set(AssetPlugin {
                        file_path: config.assets_root.to_string_lossy().into_owned(),
                        ..default()
                    })
                    .set(LogPlugin {
                        filter: config.log_filter.clone(),
                        ..default()
                    })
                    .set(WindowPlugin {
                        primary_window: Some(Window {
                            title,
                            resolution: (800, 600).into(),
                            present_mode: PresentMode::AutoVsync,
                            ..default()
                        }),
                        // The default behaviour exits the instant every window
                        // is gone, which races the SpacetimeDB disconnect
                        // `bevymmo_client::stdb::plugin::begin_shutdown` queues
                        // off the same `WindowCloseRequested` message — the
                        // process would usually die before `frame_tick` ever
                        // got to send it, leaving the player marked online
                        // until the server's presence timeout caught up.
                        // `finish_shutdown` calls `AppExit` itself once that
                        // disconnect actually goes out.
                        exit_condition: bevy::window::ExitCondition::DontExit,
                        ..default()
                    }),
            );
        }
        #[cfg(not(feature = "client"))]
        {
            let _ = config;
            panic!("windowed modes require the 'client' cargo feature to be enabled");
        }
    } else {
        let tick_duration = Duration::from_secs_f64(1.0 / config.tick_rate);
        app.add_plugins((
            MinimalPlugins.set(bevy::app::ScheduleRunnerPlugin::run_loop(tick_duration)),
            bevy::state::app::StatesPlugin,
        ));
        app.add_plugins(LogPlugin {
            filter: config.log_filter.clone(),
            ..default()
        });
        app.insert_resource(Time::<Fixed>::from_duration(tick_duration));
    }
}
