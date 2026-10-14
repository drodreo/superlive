# superlive

A component-based personal platform: one desktop shell, functions as installable components.

superlive began as a bloated monolithic personal application. The rewrite inverted it: the core now does only three things — install components, run them, and manage their lifecycle. Everything else (your todo list, knowledge base, finance dashboard, writing tools…) is a component you install only if you want it.

## What you can do with it

- **Install what you need** — drop a component archive into the window (or point the install bar at its path); the shell fingerprint-verifies every file, unpacks it, and registers it with a menu entry of its own.
- **Work in one window** — components share the sidebar menu, the workspace, the light/dark frosted-glass theme, and the interface language (中文 / English). A component built on the SDK inherits all of it for free.
- **Configure without touching files** — components declare their options; the shell renders a settings page for each (required-field validation, restart-required marking, masked secrets). No hand-edited config files, ever.
- **Control every lifecycle** — start/stop from the management page, set a per-component auto-start policy, upgrade with your configuration preserved, uninstall with zero residue (process stopped, files and data gone).
- **Stay current** — the app checks for updates and installs them itself.

## The core idea

The platform is a host; functions are guests.

- **The shell is intentionally small**: a registry, a unified API gateway, and lifecycle management. It ships no business features of its own.
- **A component is a self-contained unit**: its own backend process, its own UI, its own namespace on the gateway. It crashes alone, upgrades alone, and leaves nothing behind when uninstalled.
- **The contracts are open**: both the UI plane and the backend plane speak plain HTTP. The SDKs reduce the work, but a raw `fetch` is a fully compliant client — no lock-in anywhere.

You assemble your own platform: install the components that match how you work, remove the rest. The first generation of components — todo, knowledge, writing, documents, finance, events — live in their own repositories and serve as references.

## Why this shape

- **Lean by construction** — the shell cannot bloat with features, because it has nowhere to put them; you run exactly what you installed.
- **Real isolation** — each component is a separate OS process behind a per-component token; one component's failure or upgrade never touches another.
- **Distrust by default** — install archives are fingerprint-verified file by file; backend endpoints are assumed reachable and protected by tokens, not by obscurity.
- **Configuration as UI** — the manifest declares, the shell renders; upgrades diff the new required fields against your existing config and only ask for what is missing.
- **Small footprint** — a Rust/Tauri shell using the system webview; no bundled browser.

## Building your own component

The easiest way in: the component manager in the app has a **create** entry. A dialog collects the id, namespace, display name, which planes you want (backend process / frontend UI — the combination picks the shape: full-stack, frontend-only, or backend-only), and a project location; the shell scaffolds the complete project and registers it as a development component — the body stays in your project directory, rebuild plus stop/start picks up new artifacts (reload is the upgrade path), and uninstalling removes only the registration.

Scaffolding is handled entirely by the component manager's **Create** entry described above. What you eventually ship is an installable zip:

1. **A manifest** — id, namespace, menu/route registration, launch command, config schema, per-file fingerprints.
2. **A backend** — any language that can serve HTTP. With [slsdk-rs](slsdk-rs/) it is one line: `serve(routes)` — transport selection, token verification, the health endpoint, and the config endpoint come built in.
3. **A UI** — two tiers, declared in the manifest:
   - `module` — a React bundle dynamically loaded into the shell's route tree; via [@shell/sdk](slsdk-ts/) it inherits the theme, the language, and the shared UI facilities (tables, forms, query component families).
   - `iframe` — any stack you like, mounted in an isolated frame.

The full contract — path spaces, the lifecycle state machine, the configuration plane, the security model — is specified in [docs/architecture.md](docs/architecture.md), the authoritative architecture document. The reworked `xatodo` is the reference component.

## In this repository

- `shell/` — the desktop application (Tauri): installation, registry, gateway, UI containers, theming, i18n.
- `slsdk-rs/` — the backend SDK (Rust): the reference implementation of the component contract.
- `slsdk-ts/` — the frontend SDK (TypeScript): HTTP convenience layer + shared UI facilities.

Further docs: [docs/architecture.md](docs/architecture.md) · [docs/roadmap.md](docs/roadmap.md)
