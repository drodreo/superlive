# superlive Architecture Decisions

> This document is the distillation of the architecture discussion of 2026-09-20 and the authoritative architecture document of this repository. When it conflicts with the implementation, come back here to realign.

## Architecture in one sentence

**The shell = registry + unified API gateway + component lifecycle management.**

On the outside (Plane A) the shell exposes a single endpoint and one HTTP semantic vocabulary; components register a namespace with the registry; when a request arrives, the shell looks up the registry by the first path segment and forwards "identity + stream" as-is to the corresponding component backend. The shell never parses component business bytes — no translation layer, no private protocol.

## Three parties

| Party | What it is | Contract it faces |
|---|---|---|
| Shell | tauri desktop application: installation/deployment, registry, unified gateway, UI container | implements both planes |
| Component | independent project (e.g. the reworked xatodo): backend binary + UI static assets, distributed as an archive | Plane B (backend) + Plane A (UI) |
| SDK | slsdk-rs (Plane B, Rust) / slsdk-ts (Plane A + the Plane B `server` entry, TS) | Plane B = protocol reference implementation (shared by both ends to prevent drift); Plane A = official convenience layer (hides shell internals; the protocol stays open, no lock-in) |

One implementation concession: the ws connection of observe cannot carry custom headers, so the token is passed via the `?token=` query (an aligned trade-off).

## Plane A contract (shell ↔ component UI)

- **The protocol is plain HTTP**: a component UI may call the shell with raw fetch / any HTTP client; the SDK is not a prerequisite of the protocol.
- **Path contract (first-segment partitioning)**:
  - `/_shell/*` shell management (no token)
  - `/api/{ns}/{rest…}` component API, including ws upgrade (token required)
  - `/comps/{id}/{path…}` component UI static assets (no token — scripts/dynamic imports cannot attach custom headers, and a JS bundle is code, not user data; for installed components the URL maps to the `ui/` subdirectory of the install directory, for dev-registered components to the project's `web/dist`; nothing else from either directory ever enters the URL space)
  - The shell gateway strips `/api/{ns}` before forwarding, so component routing never knows which namespace it lives under — standalone debugging paths are identical.
- **HTTP semantic vocabulary**: `get/post/put/delete/head` are all sugar over fetch; `observe(path): Observable` alone carries real-time push (ws receive-only; the send direction goes over post), a natural fit for rxjs. The envelope carries only metadata (method/path/headers); the body is always a raw byte stream — large files stream end-to-end and are never serialized into a JSON wrapper.
- **The unified model: identity + stream.** The envelope carries only metadata (method/path/headers), the body is always a raw byte stream, and the whole chain (UI fetch → shell hyper → component) is a streaming pipeline — files are never serialized into any JSON wrapper.
  - On the TS side there is no explicit "convert to binary": objects go as JSON (on the wire that is already a UTF-8 byte stream anyway), and File/Blob/Uint8Array/ReadableStream are platform-native body types; the SDK sets Content-Type automatically by type.
  - HTTP semantics come for free: Range/206 resumable transfers, HEAD/Content-Length probing.
- **The first path segment is the namespace** (the key distributed by the registry), e.g. `/todo/list` → the `todo` component.
- **Static assets vs API split**: the component UI's JS bundles/CSS go through static paths `/comps/{id}/...` (file loading, not through the gateway); API calls go through the unified endpoint.
- **Two UI container modes** (manifest declares `ui.ui_type`):
  - `module`: the component UI is React, loaded at runtime via dynamic `import()` into the host route tree — the same experience as a monolith. Constraints: react/react-dom/i18next singletons (locked by the import map), a shared-library whitelist (react + react-dom including the jsx-runtime subpaths, rxjs, i18next, react-i18next, @shell/sdk, @shell/sdk/ui — two instances of any of them interoperate badly, same locking mechanism throughout), CSS class-prefix isolation (shared components `.sl-ui-*`, domain styles `.sl-{ns}-app`, global selectors forbidden; ?inline injection), component state self-contained and never touching host globals.
  - `iframe`: for components with heterogeneous stacks or isolation needs; src points at the shell static path.
- **@shell/sdk dual entry points** (shared UI facilities belong to the Plane A SDK, decided 2026-09-21):
  - `@shell/sdk` (api entry): the HTTP convenience layer (five verbs + observe + body adaptation + ApiError) plus the TS mirror of the dgpqp query primitive (filter types are the API request bodies).
  - `@shell/sdk/ui` (ui entry): shared UI facilities (CrudListPage/DataTable/form and query component families/hooks/utils) — the shell adaptations a component author should not have to think about (useShellState/token/CSS scoping) and the official form of the domain framework layer.
  - **CSS contract**: shared components own their styles (`.sl-ui-*` class names are the scope, low-specificity single-class selectors), injected/unloaded with ?inline as today; consumers pay zero attention to component styles; the customization channel is CSS cascade layering (domain selector + component class composition, no theme API needed, and domain-side overrides must never require `!important`). Portal-safe (the class name is the namespace; overlays mounted under body work the same way).
  - **ReferenceEntity injection table**: the data sources of the reference field type are injected by the consumer (`references?: Record<string, ReferenceSpec>`, `ReferenceSpec = { search(keyword): Observable<ReferenceOption[]>, byId?(id): Observable<ReferenceOption | undefined> }`); the schema's `reference.entity` is relaxed to a plain string; the shared hook has built-in debounce/pagination/race protection; a missing spec warns in dev mode.
  - (Historical deviation corrected: since module 4 the five components have actually implemented "?inline + prefix"; this document's original "CSS Modules" wording did not match the implementation. This revision aligns the text with reality and fixes the contract.)

## Plane B contract (shell ↔ component backend)

- **Process model**: the shell spawns an independent child process (tokio::process) — crash isolation, any language, independent upgrades. tauri has no runtime plugin mechanism, so this is the only viable shape (tauri plugins are compile-time integration; dylib has no stable ABI; wasm cannot carry a full service with sqlite listening on HTTP).
- **Registration protocol**: on install/start the component registers a namespace with the shell; routing inside the component is autonomous.
- **Transport (its own chapter, extensible)**:
  - Phase one: HTTP over UDS (Linux/macOS) / HTTP over a random 127.0.0.1 port (Windows) — tokio/mio do not support Windows UDS; platform capability checks must go down to the ecosystem level, not just the OS. The selection logic lives in the cfg branches inside slsdk-rs's serve(), invisible to component authors; the shell injects an endpoint descriptor (uds:path or tcp:addr) handled uniformly.
  - **Transport capability declaration**: runtimes without UDS support exist in the Node family (Deno has no Unix-domain-socket listener). A component declares its capability set in the manifest (`"transport": "tcponly"`); absent = every transport the host platform offers. The shell's endpoint injection honors the declaration — a tcp-only component receives a `tcp:` descriptor on every platform. The deno scaffold template declares tcp-only; the cargo and bun templates keep the default.
  - Phase two: stdio transport (the most natural shape for headless components, no port concept at all). Large files are length-prefix framed over the stdin byte pipe.
- **Lifecycle**: env injection of the listen address and token → the shell polls `GET /health` (readiness probe and health check share one endpoint) → SIGTERM graceful shutdown (Windows falls back to a stdin shutdown command) → stdout/stderr left for log aggregation, never used as a protocol channel.
- **Component author experience**: `serve(handler, manifest)` in one line — port, transport selection, token verification, and the health endpoint all live inside the SDK; the author writes only business handlers. Two reference implementations share the wire contract: slsdk-rs `serve()` for Rust backends, and the `slsdk-ts/server` entry for Node-family runtimes (node / deno / bun — runtime detection is internal, the API is identical; on a runtime lacking UDS a `uds:` descriptor is a startup error, which is why the deno template declares tcp-only). Without `SL_ENDPOINT` the serve entry falls back to standalone debug mode (`tcp:127.0.0.1:0`, unauthenticated) so a backend runs standalone for local development.

## Security

- **The token is the first wall, invariant across platforms.** Design principle: "secure even assuming the port is reachable" — every request must carry the per-component token injected at shell launch; anything without a token gets a 401.
- UDS (Unix) and Origin checks are **optional hardening** (attack-surface reduction), never a security dependency: a browser page can reach the port but cannot pass the token; a malicious process of the same user is a threat class UDS never blocked anyway (a 0700 socket only blocks other users) — both sides are equivalent.
- **No HTTPS needed**: cleartext HTTP on loopback is industry standard (the data never leaves the NIC; the threat TLS defends against does not exist), and users carry zero certificate burden.

## Installation & lifecycle (filesystem alignment)

- The install package is an archive with a fixed manifest at the root (id/version/compatible shell version/namespace/display name/ui & menu & route declarations/launch command/per-file fingerprints/config schema). `ui` and `launch` are optional — see component shapes below.
- **files are a package concept**: per-file fingerprints exist only in the package manifest, computed at publish time by the shell. A dev manifest is a pure declaration — no files list.
- **Component shapes**: both `ui` and `launch` present = full-stack; `launch` absent = frontend-only (no backend process, no gateway dispatch target for the namespace); `ui` absent = backend-only (no UI mount — routes must be empty and the menu absent, enforced by manifest validation).
- **Registry records**: the registry directory keeps one record per component — an envelope `{ source, project_dir?, manifest }`. For installed components the manifest is the copy extracted from the package (kept by the shell). For dev components the manifest field is a **snapshot projection of the project's `manifest.json`** (the authoritative declaration): loads read the project file first and fall back to the snapshot when it is unreachable (the record survives project-directory moves); start/reload re-reads the project, refreshes the snapshot, and checks source identity (e.g. the id against the backend constant). Records written before the envelope existed (bare manifests) read as installed.
- manifest.json itself is kept by the shell in the registry directory (registry/) and is not part of the install directory; **the files list must not contain manifest.json — a package that does is rejected**.
- **Compatibility gate**: at install time `min_shell_version` is compared against the current shell version; if the shell is older than the declared value the package is rejected (instead of failing later as a vague health timeout).
- After fingerprint verification the archive is unpacked into the component install directory (`components/{id}/`, binary and assets); **component UI assets always live in the `ui/` subdirectory of the package**, and the shell static service mounts only `components/{id}/ui` as the `/comps/{id}/` static space — nothing else from the install directory (binaries, assets, data files) enters the URL space.
- Upgrade atomicity: the new version is unpacked into a temp directory, verified, then swapped in by rename; **before the swap the component's config.toml is backed up and restored afterwards** (upgrades never wipe user configuration — config drift detection builds on this by diffing the new required fields); uninstall = delete the directory, and the URL space disappears with it.

### Creation flow (development registration)

- The component manager offers a creation entry with a first-step choice: **import an existing project** or **create fresh**.
- **Fresh creation**: a dialog collects the id/namespace/localized display titles (see *Localized titles* below), the plane checkboxes (backend process / frontend UI — the combination picks the shape), the backend template when a backend is checked (cargo / deno / bun — cargo generates a Rust workspace on slsdk-rs; deno and bun generate a TypeScript `server/server.ts` on the `slsdk-ts/server` entry, compiled to a self-contained binary by the template's `superlive.build`), and the project location; the shell scaffolds the project via the `slscaffold` crate (the shared base a future superlive MCP service reuses), generates the project's `manifest.json` (pure declaration per the user's input plus the built-in version convention — the author may edit it afterwards; reload picks the changes up), and registers a dev record — no package, no install step. **The version authority is the project's version source** (the backend `Cargo.toml`, the TS `server.ts` version constant, or the frontend `package.json`); the declaration's version field is a display value the shell refreshes on reload and overrides at publish time.
- **Importing an existing project**: the user picks a directory through the OS picker; the shell inspects it — manifest present and parsing, declaration validation, shape detection (server/ and web/ directories), the version-compatibility gate, and an upgrade scan. Three outcomes:
  - Fully compliant → registered as a dev entry directly (the same registration tail as fresh creation: envelope + snapshot, `registered_at` stamped now).
  - Upgradeable findings → the user is asked for confirmation with the exact fix list; confirming applies the fixes and registers, cancelling closes the dialog. **Upgrades are conservative and gap-filling only**: manifest-level corrections (derive `titles` from `display_name`, refresh `version` from the project version source, fill an empty `min_shell_version` with the current shell version) and scaffold files that are **missing** (scripts/superlive.build, README) — an existing file is never overwritten, user customizations are untouchable.
  - Unfixable findings (contract violations, a newer `min_shell_version` than the shell, an id/namespace already registered) → the reason is reported; the user may pick a different directory or close.
- **`scripts/superlive.build`** is the component's single build entry (one platform-dialect file — sh on POSIX, PowerShell on Windows; `build` is the extension): it builds whichever planes the project has and asserts the artifacts are in place. The shell's managed build/publish flow invokes it — **it is the component author's single point of build customization** (workspace layout, cross-compilation, private scripts all go here). Packaging intelligence (staging assembly, fingerprint computation, zip creation, verification) lives in the shell, not in the project.
- **Publishing** (dev → installable package): the shell invokes `superlive.build`, assembles the staging tree, computes the per-file fingerprints and writes them into the package manifest, and produces the zip. Config seeds are sanitized: fields declared `sensitive` in the config declaration are erased from the project's config.toml (default value first, empty string as fallback) before it seeds the package.
- A dev entry's static surface serves the project's `web/dist`; its spawn working directory is the project directory, with a built-artifact precheck (the start command reports a build-first error; shell-restart recovery skips the entry with a warning). The development loop is `vite build --watch` / `cargo build` + stop → start — the shell re-reads the record on start, so **reload is the upgrade path for dev entries** (backup-and-swap upgrades are an installed-only concept).
- Uninstalling a dev entry removes the registration only — the project directory is never touched. A dev component places **no file** in the installed-components area (`components/{id}/`).

### Localized titles (menu & route)

- Menu and route titles are localized. `MenuDeclaration` and `RouteDeclaration` carry `title` (the single-language fallback, present in every manifest) plus `titles` (an optional locale → string map; absent or empty = a single-language entry — every legacy manifest and package stays valid unchanged).
- **The menu title resource lives in the component's own i18n namespace** under the fixed key `menuTitle`. At registration/install the shell injects the manifest's `titles` (with `title` as the fallback value) into the host i18next instance beside the component's own registered resources — one namespace holds everything the component surfaces to the host, so cross-component key collisions are structurally impossible.
- **Fallback chain**: the requested locale → `zh` → `en` → `title` — resolved per lookup; the host's own global fallback configuration is untouched.
- **`display_name` is derived at assembly time**: `titles['zh'] ?? titles['en'] ?? first entry` — every consumer of display_name (the manager list, fallback surfaces) is localized through the same chain.
- **The creation dialog collects localized titles**: locale-selector + title-input rows (zh and en seeded by default, user-addable). The scaffolder fills the generated project's i18n module with the same values — one registration line per configured locale — so the component's in-page title is localized from the first minute.
- **Scaffold namespace constraint**: the generated project's i18n registration uses the component's namespace, pre-wired by the scaffolder. The template carries an English note stating the constraint (the namespace is reserved for the component, the menu-title key is shell-managed) and where to add the project's own keys — visible to the author and to any AI assistant working on the project.

## Configuration plane (component option configuration contract, decided 2026-09-22)

The component's option configuration (the config.toml layer) becomes UI: components declare config items, the shell provides the unified settings interface, and users never hand-edit config files.

### Three laws of the lifecycle

1. **Registration ≠ start**: registration = entering shell management (visible/configurable/startable); process start is a separate next stage.
2. **Configuration precedes start**: after first install, when the user chooses "start now" → the shell enters the settings page driven by the manifest-declared options → configuration completes and validates → only then is the component backend launched. All defaults with zero required fields = a zero-config component that can start right after install.
3. **Upgrade drift detection**: when a config file already exists (the main scenario being a component upgrade), the **new version's required fields are diffed against the existing config.toml** — missing required fields go to the settings page; if nothing is missing, start directly.

### Config declaration (static manifest declaration)

- The manifest `config` section declares the item list; each item: key, label (an i18n key), type (string/number/boolean/select/datetime), default value, required, option enumeration, format constraints (e.g. cron), restart_required, sensitive flag (display masked), grouping.
- **Why static declaration**: the settings page works while the component process is not yet running, which rules out runtime reporting. Runtime endpoints only read/write **values**.
- **i18n travels with the UI package**: the manifest points at static JSON (`config_i18n: { zh, en }`); the files sit in the component's `ui/` directory and ship with the package (fetchable via the `/comps/{id}/` static space) — the settings page does not depend on the component's ESM module loading successfully.

### Config read/write

- **The persistent form is the component's config.toml** (the single source of truth), located in the component's working directory — the install directory for installed components, the project directory for dev components (the same workdir the spawn uses; config follows the component body). Two writers: when not running, the shell writes the file directly; while running, the component's standard endpoint is used.
- **Standard endpoint**: `GET/PUT /api/{ns}/config` (forwarded by the shell gateway). PUT = dynamic toml write-back (unknown sections preserved) + hot reload of runtime items (generalized from the sl-finance precedent, sunk into the SDK). slsdk-rs's serve() ships this endpoint built in — a component author who declares config items gets it at zero cost.
- **Effect semantics**: restart_required items are applied by the shell restarting the component; runtime items are hot-reloaded by the component.
- **Validation**: schema level only in v1 (required/type/format), executed on the shell side; connectivity probing is out of scope for v1.
- **Startup essentials (database_url/home etc.) are neither declared nor shown in v1**: a wrong edit = the component cannot start, and database-switch migration semantics are undefined; a readonly display capability is left for a later version. The manifest config section declares only editable items (tokens/toggles) — this is the main value of the UI-ization.

### Launch-option slot

- After configuration completes and validation passes, **the shell steps in with a "launch options" prompt** — currently one item: the start policy (auto/manual). The policy persists per component in the **shell-side registry** (a runtime user choice; it is shell management data and never goes into the component manifest).
- This intervention point is an **extensible slot**: future launch options hook in here without touching the main lifecycle flow.
- On the next shell start, components are launched per their policy; components in the needs-config state are held at the configuration gate.

### Component state machine

`installed → needs-config (missing required fields) / ready (zero-config) → (configuration complete and validated) → launch-option prompt → starting → running → stopped`; after an upgrade, any state may fall back to needs-config.

### Legacy adaptation

- sl-finance: standardized onto this contract (endpoint existed; manifest declaration added; hot-reload logic sunk into the SDK with serve()).
- events/knowledge/writing/documents: demand-driven, untouched in this batch (config is startup essentials only, displayed read-only).
- xatodo: the multi-user config layer (users/apikey) is out of scope for this batch.

## Decision record (key trade-offs)

1. **Reverse proxy vs unified API** → unified API. Under a reverse proxy the shape of Plane A is dictated by Plane B (contract bleed-through); with a unified API, the Plane A contract = the HTTP semantic vocabulary and the Plane B contract = registration protocol + transport; the two planes meet only at the registry, and even a stdio transport is absorbed (the shell's dispatch layer converts).
2. **stdio vs HTTP (shell↔component)** → HTTP semantics first. stdio concentrates translation complexity in the shell (ws bridging / framing / routing conventions — all private-protocol invention); HTTP semantics require zero invention. stdio is kept as the phase-two transport for headless components.
3. **JSON envelope vs HTTP metadata as the envelope** → the latter. The envelope carries only metadata, the body is always a byte stream, and the large-file problem does not exist at the protocol level (a JSON envelope inflates base64 by 33% + loads everything into memory).
4. **Dynamic UI loading** → native ESM + import map (a React component is just a function, no assembly ceremony; the import map locks the react singleton against double-instance crashes); Module Federation is overweight for an in-house component set.
5. **UI layout** → the `ui/` subdirectory inside the component package; the shell mounts only that subdirectory as the `/comps/{id}/` static space (lifecycle aligned with the filesystem: uninstall deletes the whole install directory, upgrade swaps UI and binary in one atomic rename; the exposed surface shrinks to the UI subtree — binaries/assets never enter the URL space). The webview cannot read local files (under file:// ESM is rejected by CORS), so the bridge is the shell's local HTTP static service; a tauri custom protocol is not used (inconsistent scheme shapes across platforms, and ESM/import map/ws behavior would each need separate workarounds).
6. **API path shape** → a unified `/api` prefix (not a fallback catch-all): explicit path-space partitioning (the first URL segment is the traffic type), a positive auth rule (only /api/ verifies the token), and a one-sentence UI contract ("APIs always start with /api").
7. **Keep or drop the Plane A SDK** → keep, positioned as the official convenience layer: its job is to reduce developer cognitive load and hide shell internals (token/prefix/body adaptation/errors/ws reconnect, including the fetch façade); the protocol stays open with no lock-in.

## The first component

The reworked xatodo: the server (Rust) + web (React) shape is naturally the canonical form of "Plane B backend + Plane A UI", serving as the first validation case of the component mechanism.
