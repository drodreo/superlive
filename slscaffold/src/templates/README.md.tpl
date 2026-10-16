# __NAME__

#@if IMPORTED
A [superlive](https://github.com/drodreo/superlive) component (an existing project
registered with the superlive shell).
#@endif
#@if !IMPORTED
A [superlive](https://github.com/drodreo/superlive) component scaffolded by `slscaffold`.
#@endif

#@if HAS_SERVER && BACKEND_CARGO
- **server/** — Rust backend (`__ID__-server`). The `slsdk-rs` serve() skeleton handles
  listening, token auth, the `/health` endpoint and SIGTERM graceful exit; you only write
  business handlers.
#@endif
#@if HAS_SERVER && BACKEND_DENO
- **server/** — Deno backend (`server/server.ts`, TypeScript). The `slsdk-ts` serve()
  skeleton handles listening, token auth and the `/health` endpoint; you only write
  business handlers. The shell writes the project's `manifest.json` at registration, and
  that declaration carries tcp-only transport (the Deno runtime has no Unix domain socket
  support), so the shell injects a loopback TCP port on every platform.
#@endif
#@if HAS_SERVER && BACKEND_BUN
- **server/** — Bun backend (`server/server.ts`, TypeScript). The `slsdk-ts` serve()
  skeleton handles listening, token auth and the `/health` endpoint; you only write
  business handlers.
#@endif
#@if HAS_WEB
- **web/** — React frontend (module tier). Builds to a fixed-name `entry.js` whose default
  export is the component root; shared libraries (react / rxjs / i18next) stay external and
  are resolved by the shell's import map at runtime — app-wide singletons. Localized UI
  strings live in `web/src/i18n.ts` (one bundle per locale): extend your own keys there per
  locale — a shell language switch applies instantly; the `menuTitle` key of the component
  namespace is reserved for the shell (see the note in that file).
#@endif
- **scripts/** — build entry: `superlive.build` runs the builds for every plane this
  project carries (one command, shape-adaptive); release packaging is handled by
  the superlive shell's managed flow, not by this project.

## Prerequisites

#@if HAS_SERVER && BACKEND_CARGO
- Rust toolchain (stable)
#@endif
#@if HAS_SERVER && BACKEND_DENO
- Deno (https://deno.com)
#@endif
#@if HAS_SERVER && BACKEND_BUN
- Bun (https://bun.sh)
#@endif
#@if HAS_WEB
- Node.js + pnpm (or npm)
#@endif
- The superlive shell (for installation)

#@if !IMPORTED
## Develop

#@endif
#@if !IMPORTED && HAS_SERVER && HAS_WEB && BACKEND_CARGO
Terminal 1 — backend (fixed port so the dev proxy can reach it):

    SL_ENDPOINT=tcp:127.0.0.1:__DEV_PORT__ cargo run --manifest-path server/Cargo.toml

#@endif
#@if !IMPORTED && HAS_SERVER && HAS_WEB && BACKEND_DENO
Terminal 1 — backend (first time: `cd server && bun install` — deno needs a
pre-installed node_modules for file: dependencies; npm works too):

    SL_ENDPOINT=tcp:127.0.0.1:__DEV_PORT__ deno run --node-modules-dir=manual --allow-net --allow-read --allow-env server/server.ts

#@endif
#@if !IMPORTED && HAS_SERVER && HAS_WEB && BACKEND_BUN
Terminal 1 — backend (fixed port so the dev proxy can reach it):

    SL_ENDPOINT=tcp:127.0.0.1:__DEV_PORT__ bun run server/server.ts

#@endif
#@if !IMPORTED && HAS_SERVER && HAS_WEB
Terminal 2 — frontend:

    cd web && pnpm install && pnpm dev

Open http://localhost:5173 — in standalone mode `main.tsx` fakes the shell injection
(empty origin) and the vite dev proxy forwards `/api` to the backend, so the full
UI → API → handler chain works outside the shell.

> Inside the shell, the gateway forwards `/api/__NS__/*` to this backend with the namespace
> segment stripped; the component always receives its own full paths (`/api/v1/hello`) —
> identical in both worlds.
#@endif
#@if !IMPORTED && HAS_WEB && !HAS_SERVER
    cd web && pnpm install && pnpm dev

Open http://localhost:5173 — in standalone mode `main.tsx` fakes the shell injection
(empty origin), so the UI renders outside the shell. This shape has no backend: the
vite dev proxy target is a placeholder, and API calls fail unless something else
serves this component's namespace.
#@endif
#@if !IMPORTED && HAS_SERVER && !HAS_WEB && BACKEND_CARGO
Run the backend standalone (fixed port; auth is only enforced when launched by the
shell):

    SL_ENDPOINT=tcp:127.0.0.1:__DEV_PORT__ cargo run --manifest-path server/Cargo.toml

Inside the shell, the gateway forwards `/api/__NS__/*` to this backend with the namespace
segment stripped; the backend always receives its own full paths (`/api/v1/hello`).
#@endif
#@if !IMPORTED && HAS_SERVER && !HAS_WEB && BACKEND_DENO
Run the backend standalone (first time: `cd server && bun install` — deno needs a
pre-installed node_modules for file: dependencies; npm works too; fixed port,
auth is only enforced when launched by the shell):

    SL_ENDPOINT=tcp:127.0.0.1:__DEV_PORT__ deno run --node-modules-dir=manual --allow-net --allow-read --allow-env server/server.ts

Inside the shell, the gateway forwards `/api/__NS__/*` to this backend with the namespace
segment stripped; the backend always receives its own full paths (`/api/v1/hello`).
#@endif
#@if !IMPORTED && HAS_SERVER && !HAS_WEB && BACKEND_BUN
Run the backend standalone (fixed port; auth is only enforced when launched by the
shell):

    SL_ENDPOINT=tcp:127.0.0.1:__DEV_PORT__ bun run server/server.ts

Inside the shell, the gateway forwards `/api/__NS__/*` to this backend with the namespace
segment stripped; the backend always receives its own full paths (`/api/v1/hello`).
#@endif

## Build

    scripts/superlive.build

(On Windows: `pwsh -File scripts/superlive.build`.)

Builds every plane this project carries — the backend release binary and the web
dist with the fixed-name `entry.js` — and asserts the outputs exist. This script
is your single build customization point: if your component needs a custom build
(workspace layout, cross-compilation, private build steps), change it. Release
packaging (assembly, fingerprinting, the install zip) is handled by the superlive
shell's managed build/package flow, which invokes this script.

## Install

In the superlive shell: open the component manager (sidebar bottom) → install the
component. The shell's managed build/package flow invokes `scripts/superlive.build`,
registers the component and mounts its menu entry and route.

#@if !IMPORTED
## SDK dependency mode

__DEP_MODE_DESC__
#@endif
