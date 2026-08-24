#!/usr/bin/env bash
#
# Wrapper around the `spacetime` CLI for this repo.
#
# It exists for two reasons: the module lives in a subdirectory and is outside
# the cargo workspace, and `spacetime generate` needs both an explicit
# `--module-path` and an out-dir that nobody should have to remember. Every
# command here is one you would otherwise mistype.
#
#   ./scripts/stdb.sh publish          build + publish to the local server
#   ./scripts/stdb.sh generate         regenerate the Bevy client's Rust bindings
#   ./scripts/stdb.sh generate-gateway regenerate the gateway's Rust bindings
#   ./scripts/stdb.sh dev               watch, rebuild, republish, regenerate
#   ./scripts/stdb.sh logs             tail module logs
#   ./scripts/stdb.sh sql "..."        run a SQL query against the module
#   ./scripts/stdb.sh reset            wipe the database and republish from scratch
#
# The server itself comes from docker: `docker compose up -d spacetimedb`.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MODULE_PATH="$REPO_ROOT/crates/stdb-module"
BINDINGS_OUT="$REPO_ROOT/crates/client/src/stdb/module_bindings"
GATEWAY_BINDINGS_OUT="$REPO_ROOT/apps/gateway/src/stdb/module_bindings"
SERVER="${STDB_SERVER:-local}"
# Local development database. Matches `spacetime_module` in
# `config/default.toml`, which is what a locally-run client connects to.
#
# Production publishes a *different* name (`bevymmo-v2`, set via
# `BEVYMMO__SPACETIME_MODULE` in `docker-compose.yml`) to a different host, so
# the two are not meant to agree — but nothing said so, and reading the two
# files side by side looks like a bug. Override both here when pointing this
# script at something other than the local server:
#
#   STDB_SERVER=https://stdb.example.com STDB_DATABASE=bevymmo-v2 ./scripts/stdb.sh logs
DATABASE="${STDB_DATABASE:-bevymmo}"

usage() {
    sed -n '3,18p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
    exit "${1:-0}"
}

case "${1:-}" in
publish)
    exec spacetime publish -s "$SERVER" --module-path "$MODULE_PATH" "$DATABASE"
    ;;
generate)
    exec spacetime generate --lang rust \
        --module-path "$MODULE_PATH" \
        --out-dir "$BINDINGS_OUT"
    ;;
generate-gateway)
    # The gateway gets its own copy rather than depending on
    # `bevymmo_client` for these: that crate pulls in all of Bevy, which a
    # server process has no business linking. Generated code, so
    # duplicating it costs nothing beyond running this command twice.
    exec spacetime generate --lang rust \
        --module-path "$MODULE_PATH" \
        --out-dir "$GATEWAY_BINDINGS_OUT"
    ;;
dev)
    # `dev` resolves the bindings dir relative to --project-path, so the project
    # is the repo root and both paths below are relative to it.
    exec spacetime dev -s "$SERVER" \
        --project-path "$REPO_ROOT" \
        --module-path "$MODULE_PATH" \
        --module-bindings-path crates/client/src/stdb/module_bindings \
        --client-lang rust \
        "$DATABASE"
    ;;
logs)
    shift
    exec spacetime logs -s "$SERVER" "$DATABASE" "$@"
    ;;
sql)
    shift
    [ $# -ge 1 ] || { echo "stdb.sh sql: missing query" >&2; exit 2; }
    exec spacetime sql -s "$SERVER" "$DATABASE" "$@"
    ;;
reset)
    # `init` only runs against an empty database, so re-seeding the world after
    # a schema change means wiping first. Destructive on purpose: every
    # character and every bit of world state goes.
    exec spacetime publish -s "$SERVER" --module-path "$MODULE_PATH" \
        --delete-data -y "$DATABASE"
    ;;
-h | --help | help) usage 0 ;;
*) usage 2 ;;
esac
