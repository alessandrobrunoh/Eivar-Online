# BevyMMO Agent Guide

This document contains high-signal, repo-specific facts to help agents avoid mistakes and understand non-obvious conventions in this repository.

## Commands & Workflows

**Building & Running**
The authoritative server is a **SpacetimeDB module**, not a Bevy process. The `game` binary is a client and nothing else.

1. `docker compose up -d spacetimedb` — starts the database on `:3000`
2. `./scripts/stdb.sh publish` — builds the WASM module and publishes it
3. `cargo run -- client` — runs the game

Other module commands: `./scripts/stdb.sh {generate,dev,logs,sql,reset}`. `dev` watches, rebuilds, republishes and regenerates client bindings on every change; `reset` wipes the database and re-seeds it, which is what a schema change needs because `init` only runs against an empty database.

**Testing & Verification**
- **Test**: `cargo test --workspace`
- **Lint**: `cargo clippy --workspace --all-targets -- -D warnings`
- **Module**: `cd crates/stdb-module && spacetime build`

## Database & Environment

There is no separate database: SpacetimeDB tables *are* the authoritative state, the persistence and the replication, all at once. No migrations to write — schema changes are applied automatically on publish, and only a change that cannot be migrated automatically needs `./scripts/stdb.sh reset`.

- **Local Setup**: `docker compose up -d spacetimedb`, then `./scripts/stdb.sh publish`.
- **Configuration**: layered `config/default.toml` <- `config/<APP_ENV>.toml` <- `config/local.toml` <- ENV VARS, `APP_ENV` defaults to `development`. The client needs `spacetime_uri` and `spacetime_module`; both have working defaults. Do not commit secrets; place them in `config/local.toml` (gitignored).
- **Auth**: a connection's `Identity` is issued and verified by SpacetimeDB, and the character is keyed by it. The token is cached client-side, so deleting it means a new identity and therefore a new character.

## The SpacetimeDB module

`crates/stdb-module` is **deliberately excluded from the Cargo workspace**. It compiles to `wasm32-unknown-unknown` as a `cdylib`, and `spacetimedb-bindings-sys` declares its host functions as WASM imports with no `#[cfg(target_arch)]` guard — building it for the host leaves them unresolved and the link fails. `cargo check --workspace` passes anyway (it does not link), so the failure would only surface on `cargo build`. Build it with `spacetime build`, never `cargo build`.

Three constraints inside the module that the compiler will not remind you about:

- **No filesystem, no wall clock, no OS RNG.** `ctx.timestamp` (a field, not a method) and `ctx.rng()`. Map data is pre-compiled by `build.rs` and embedded.
- **Every table persists**, including tables modelling transient state. Runtime tables are cleared and re-seeded in `init` — but `init` only fires against an *empty* database, so a plain `publish` over a live one inherits mid-flight projectiles, casts and threat. Call the GM-gated `gm_reset_runtime_state` reducer afterwards when that matters; it clears and re-seeds the transient half and leaves every character alone.
- **A tick is one transaction, single-threaded.** What used to be several Bevy systems is now ordered calls in `tick::game_tick`.

Table row types must have **named fields**: the `SpacetimeType` derive panics on tuple structs (`sats.rs` does `f.ident.unwrap()`). This is why `bevymmo_domain`'s newtypes are mirrored in `crates/stdb-module/src/rows.rs` rather than stored directly.

## Architecture & Code Conventions

- **Workspace split**:
  - `bevymmo_domain` = game rules and data, **no Bevy**, compiles to WASM. The client and the module both use it.
  - `bevymmo_shared` = the Bevy-facing layer: components, resources, world loading from disk
  - `bevymmo_client` = connection to SpacetimeDB, input, targeting
  - `bevymmo_presentation` = rendering, scenes, UI
  - `crates/stdb-module` = the authoritative server (outside the workspace)
  - `bins/game` = composition root CLI binary (client only)

- **Where does this type go?** If a rule has to run on the server it belongs in `bevymmo_domain`, because the server is a WASM module that cannot link Bevy. Bevy derives there are behind the `bevy` feature, which only `bevymmo_shared` turns on. If it is about rendering, input or the ECS, it belongs in `bevymmo_shared` or above.

- **One implementation of a rule, not two.** `bevymmo_domain::movement::step_towards` is called by the module's tick *and* by the client's dead reckoning. Two hand-written versions of the same rule diverge in exactly the way that makes a character rubber-band. The same applies to spell casting: `SpellCastContext` collects pending effects and both sides drain them.
- **Client Presentation**: The client's renderer creates visual representation by reading the replicated state and adding local components (like `Mesh3d`, materials, and `Transform`). UI widgets similarly build local views of replicated gameplay state.
- **Docs**: Read `docs/architecture.md`, `docs/database.md`, and `docs/create-a-new-plugin.md` for deeper structural guidelines.
