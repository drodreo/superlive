#!/usr/bin/env bash
# __NAME__ → superlive component build entry.
#
# superlive's managed build/package flow (inside the shell) invokes this script
# to build the component's planes; the shell owns assembly, fingerprinting and
# packaging — this script's job ends when the build outputs exist.
#
# This is your single build customization point: if your component needs a
# custom build (workspace layout, cross-compilation, private build steps),
# change this script.
#
# Form adaptation: the build sections below check the directory layout at
# runtime — a server/ directory triggers the backend build, a web/ directory
# triggers the frontend build.
#
# Requires:
#@if HAS_SERVER && BACKEND_CARGO
#   (server build) cargo
#@endif
#@if HAS_SERVER && BACKEND_DENO
#   (server build) deno (https://deno.com), plus bun or npm for the
#   node_modules pre-install (deno cannot resolve file: deps alone)
#@endif
#@if HAS_SERVER && BACKEND_BUN
#   (server build) bun (https://bun.sh)
#@endif
#@if HAS_WEB
#   (web build) pnpm (or npm)
#@endif

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

log() { echo "==> $*"; }
die() { echo "error: $*" >&2; exit 1; }

HAS_SERVER_DIR=0
HAS_WEB_DIR=0
#@if HAS_SERVER
if [ -d "$ROOT/server" ]; then HAS_SERVER_DIR=1; fi
#@endif
#@if HAS_WEB
if [ -d "$ROOT/web" ]; then HAS_WEB_DIR=1; fi
#@endif

if [ "$HAS_SERVER_DIR" -eq 0 ] && [ "$HAS_WEB_DIR" -eq 0 ]; then
  die "nothing to build: neither server/ nor web/ exists under $ROOT"
fi

#@if HAS_SERVER && BACKEND_CARGO
if [ "$HAS_SERVER_DIR" -eq 1 ]; then
  command -v cargo >/dev/null 2>&1 || die "missing required command: cargo"
  log "building server: cargo build --release"
  (cd "$ROOT/server" && cargo build --release)
  SERVER_BIN="$ROOT/server/target/release/__ID__-server"
  [ -f "$SERVER_BIN" ] || die "release binary does not exist: $SERVER_BIN"
fi
#@endif
#@if HAS_SERVER && BACKEND_DENO
if [ "$HAS_SERVER_DIR" -eq 1 ]; then
  command -v deno >/dev/null 2>&1 || die "missing required command: deno - install it from https://deno.com first"
  # Deno cannot resolve file: dependencies by itself: pre-install node_modules
  # with bun (native file: support, recommended) or npm, then compile with
  # --node-modules-dir=manual. The --allow flags are baked into the compiled
  # binary (the shell spawns it as a plain executable, with no deno flags).
  if [ ! -d "$ROOT/server/node_modules" ]; then
    if command -v bun >/dev/null 2>&1; then
      log "installing server deps: bun install (deno needs a pre-installed node_modules)"
      (cd "$ROOT/server" && bun install) || die "server dependency pre-install failed (bun)"
    elif command -v npm >/dev/null 2>&1; then
      log "installing server deps: npm install (deno needs a pre-installed node_modules)"
      (cd "$ROOT/server" && npm install) || die "server dependency pre-install failed (npm)"
    else
      die "missing required command: bun (or npm) - needed to pre-install server/node_modules for deno"
    fi
  fi
  log "building server: deno compile --node-modules-dir=manual --allow-net --allow-read --allow-env --output bin/__ID__-server server/server.ts"
  (cd "$ROOT" && deno compile --node-modules-dir=manual --allow-net --allow-read --allow-env --output "bin/__ID__-server" server/server.ts)
  SERVER_BIN="$ROOT/bin/__ID__-server"
  [ -f "$SERVER_BIN" ] || die "release binary does not exist: $SERVER_BIN"
fi
#@endif
#@if HAS_SERVER && BACKEND_BUN
if [ "$HAS_SERVER_DIR" -eq 1 ]; then
  command -v bun >/dev/null 2>&1 || die "missing required command: bun - install it from https://bun.sh first"
  log "building server: bun build --compile --outfile bin/__ID__-server server/server.ts"
  (cd "$ROOT" && bun build --compile --outfile "bin/__ID__-server" server/server.ts)
  SERVER_BIN="$ROOT/bin/__ID__-server"
  [ -f "$SERVER_BIN" ] || die "release binary does not exist: $SERVER_BIN"
fi
#@endif

#@if HAS_WEB
if [ "$HAS_WEB_DIR" -eq 1 ]; then
  # pnpm when available, npm otherwise
  PM="$(command -v pnpm >/dev/null 2>&1 && echo pnpm || echo npm)"
  command -v "$PM" >/dev/null 2>&1 || die "missing required command: pnpm (or npm)"
  log "building web: $PM run build (output web/dist)"
  (cd "$ROOT/web" && "$PM" run build)
  # Component build-shape contract (web/vite.config.js): the entry is the fixed
  # name entry.js (single rollup input src/entry.tsx + entryFileNames 'entry.js');
  # index.html never enters the build output.
  [ -f "$ROOT/web/dist/entry.js" ] || die "incomplete web build output: web/dist/entry.js does not exist"
fi
#@endif

log "build complete"
