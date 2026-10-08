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
| SDK | slsdk-rs (Plane B) / slsdk-ts (Plane A) | Plane B = protocol reference implementation (shared by both ends to prevent drift); Plane A = official convenience layer (hides shell internals; the protocol stays open, no lock-in) |

One implementation concession: the ws connection of observe cannot carry custom headers, so the token is passed via the `?token=` query (an aligned trade-off).

## Plane A contract (shell ↔ component UI)

- **The protocol is plain HTTP**: a component UI may call the shell with raw fetch / any HTTP client; the SDK is not a prerequisite of the protocol.
- **Path contract (first-segment partitioning)**:
  - `/_shell/*` shell management (no token)
  - `/api/{ns}/{rest…}` component API, including ws upgrade (token required)
  - `/comps/{id}/{path…}` component UI static assets (no token — scripts/dynamic imports cannot attach custom headers, and a JS bundle is code, not user data; the URL maps to the `ui/` subdirectory of the component install directory; nothing else from the install directory ever enters the URL space)
  - The shell gateway strips `/api/{ns}` before forwarding, so component routing never knows which namespace it lives under — standalone debugging paths are identical.
- **HTTP semantic vocabulary**: `get/post/put/delete/head` are all sugar over fetch; `observe(path): Observable` alone carries real-time push (ws receive-only; the send direction goes over post), a natural fit for rxjs. The envelope carries only metadata (method/path/headers); the body is always a raw byte stream — large files stream end-to-end and are never serialized into a JSON wrapper.
- **The unified model: identity + stream.** The envelope carries only metadata (method/path/headers), the body is always a raw byte stream, and the whole chain (UI fetch → shell hyper → component) is a streaming pipeline — files are never serialized into any JSON wrapper.
  - On the TS side there is no explicit "convert to binary": objects go as JSON (on the wire that is already a UTF-8 byte stream anyway), and File/Blob/Uint8Array/ReadableStream are platform-native body types; the SDK sets Content-Type automatically by type.
  - HTTP semantics come for free: Range/206 resumable transfers, HEAD/Content-Length probing.
- **The first path segment is the namespace** (the key distributed by the registry), e.g. `/todo/list` → the `todo` component.
- **Static assets vs API split**: the component UI's JS bundles/CSS go through static paths `/comps/{id}/...` (file loading, not through the gateway); API calls go through the unified endpoint.
- **Two UI container modes** (manifest declares `ui.type`):
  - `module`: the component UI is React, loaded at runtime via dynamic `import()` into the host route tree — the same experience as a monolith. Constraints: react/react-dom singletons (locked by import map), a shared-library whitelist (react + react-dom + rxjs + @shell/sdk — the SDK's observe depends on rxjs; two rxjs instances interoperating would blow up, same locking mechanism as react), CSS class-prefix isolation (shared components `.sl-ui-*`, domain styles `.sl-{ns}-app`, global selectors forbidden; ?inline injection), component state self-contained and never touching host globals.
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
  - Phase two: stdio transport (the most natural shape for headless components, no port concept at all). Large files are length-prefix framed over the stdin byte pipe.
- **Lifecycle**: env injection of the listen address and token → the shell polls `GET /health` (readiness probe and health check share one endpoint) → SIGTERM graceful shutdown (Windows falls back to a stdin shutdown command) → stdout/stderr left for log aggregation, never used as a protocol channel.
- **Component author experience**: `serve(routes)` in one line — port, transport selection, token verification, and the health endpoint all live inside the SDK; the author writes only business handlers.

## Security

- **The token is the first wall, invariant across platforms.** Design principle: "secure even assuming the port is reachable" — every request must carry the per-component token injected at shell launch; anything without a token gets a 401.
- UDS (Unix) and Origin checks are **optional hardening** (attack-surface reduction), never a security dependency: a browser page can reach the port but cannot pass the token; a malicious process of the same user is a threat class UDS never blocked anyway (a 0700 socket only blocks other users) — both sides are equivalent.
- **No HTTPS needed**: cleartext HTTP on loopback is industry standard (the data never leaves the NIC; the threat TLS defends against does not exist), and users carry zero certificate burden.

## Installation & lifecycle (filesystem alignment)

- The install package is an archive with a fixed manifest at the root (id/version/compatible shell version/namespace/menu and route registration declarations/launch command/per-file fingerprints/config schema).
- manifest.json itself is kept by the shell in the registry directory (registry/) and is not part of the install directory; **the files list must not contain manifest.json — a package that does is rejected**.
- **Compatibility gate**: at install time `min_shell_version` is compared against the current shell version; if the shell is older than the declared value the package is rejected (instead of failing later as a vague health timeout).
- After fingerprint verification the archive is unpacked into the component install directory (`components/{id}/`, binary and assets); **component UI assets always live in the `ui/` subdirectory of the package**, and the shell static service mounts only `components/{id}/ui` as the `/comps/{id}/` static space — nothing else from the install directory (binaries, assets, data files) enters the URL space.
- Upgrade atomicity: the new version is unpacked into a temp directory, verified, then swapped in by rename; **before the swap the component's config.toml is backed up and restored afterwards** (upgrades never wipe user configuration — config drift detection builds on this by diffing the new required fields); uninstall = delete the directory, and the URL space disappears with it.

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

- **The persistent form is the component's config.toml** (the single source of truth). Two writers: when not running, the shell writes the file directly (the install directory belongs to the shell); while running, the component's standard endpoint is used.
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
