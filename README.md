# superlive

A component-based desktop application platform: a tauri shell plus installable components. The motivation came from the bloated superlive monolith — the core keeps only component installation, deployment, and lifecycle management; all business functionality lives in components, and users install only what they need.

## Repository Layout

- `slsdk-rs/` — **Plane B SDK (Rust)**: the reference implementation of the component backend contract. Manifest types, the registration protocol (namespace declaration), lifecycle (env injection / health / graceful shutdown), and the serve() skeleton (HTTP over UDS/TCP; stdio transport in phase two). The shell and the components share the same crate, so the contract cannot drift.
- `slsdk-ts/` — **Plane A SDK (TypeScript)**: the official convenience layer — its job is to reduce developer cognitive load and hide shell internals: token injection, the /api prefix, body type adaptation (object/file/stream with automatic Content-Type), unified errors (ApiError), and observe streaming with reconnect. The protocol is open: raw fetch or any HTTP client is equally compliant.
- `shell/` — **The shell application (tauri)**: component installation and deployment (archive + manifest + fingerprint verification), the registry, the unified API gateway (identity + stream forwarding), UI container mounting (module/iframe), and menu-route registration.

## Documentation

- [docs/architecture.md](docs/architecture.md) — architecture decisions and the decision record (the authoritative architecture document of this repository)

## Components

Components (such as the reworked xatodo) are independent projects and live outside this repository — they are distributed as archives, installed by the shell, launched by the shell, and registered with the registry.
