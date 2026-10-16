# AGENTS.md

Guidance for AI agents (and fellow developers) working **on superlive itself**. For the user-facing project description see [README.md](README.md); for the authoritative architecture see [docs/architecture.md](docs/architecture.md).

## Repository layout

- `slsdk-rs/` — **Plane B SDK (Rust)**: the reference implementation of the component backend contract. Manifest types, the registration protocol (namespace declaration), lifecycle (env injection / health / graceful shutdown), and the serve() skeleton. The shell and the components share the same crate, so the contract cannot drift.
- `slsdk-ts/` — **Plane A SDK (TypeScript)**: the official convenience layer — its job is to reduce developer cognitive load and hide shell internals: token injection, the /api prefix, body type adaptation (object/file/stream with automatic Content-Type), unified errors (ApiError), and observe streaming with reconnect. The protocol is open: raw fetch or any HTTP client is equally compliant.
- `shell/` — **The shell application (tauri)**: component installation and deployment (archive + manifest + fingerprint verification), the registry, the unified API gateway (identity + stream forwarding), UI container mounting (module/iframe), menu-route registration, theming, and i18n. Registry records are envelopes (`{ source, project_dir?, manifest }`); dev entries (created by the in-app creation flow) keep their body in the project directory — static surface serves the project's `web/dist`, spawn cwd is the project dir, uninstall removes the registration only.
- `slscaffold/` — **Component project scaffolding**: the crate behind the in-app create flow (and the reuse base for a future superlive MCP service). Zero runtime deps; templates embedded via `include_str!` and shape-conditioned with whole-line `#@if EXPR` / `#@endif` markers.

Components are independent projects and live outside this repository — they are distributed as archives, installed by the shell, launched by the shell, and registered with the registry.

## Working rules for agents

- **docs/architecture.md is the contract authority** — when the implementation conflicts with it, stop and realign there first; never improvise contract semantics in code.
- **Language**: English throughout (docs, comments, commit messages); UI strings go through i18next (`shell/src/i18n.ts`, the `shell` namespace — typed keys, zh/en parity compile-enforced).
- **Gates**: see [.github/workflows/ci.yml](.github/workflows/ci.yml) — rust test on a three-platform matrix (fmt/clippy on Linux; the toolchain is pinned by `rust-toolchain.toml`), web format/typecheck/test (slsdk-ts) and build/format/smoke (shell). The shell uses pnpm; tauri CLI invocations are `cargo tauri …`.
- **Vendor shims** (`shell/src/vendor/*.ts`): the react-family dev shims are generated — regenerate with `node scripts/gen-vendor-shims.mjs` after a react/react-dom bump; never hand-edit.
- **Scaffold templates** (`slscaffold/src/templates/`): adding `#@if` markers to a template also requires flipping its `FilePlan` entry to `conditional: true` — substitution-only is the plan default, and markers silently survive it.
- **Component i18n/theme contract**: module-tier components share the shell's i18next instance (via the import-map whitelist) and register namespaces with `registerI18n` from `@shell/sdk/ui`; the shell owns the `--sl-ui-*` token namespace (`html[data-theme]` mappings in `shell/src/App.css`).
