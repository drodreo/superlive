//! ServerManifest: the ts mirror of the slsdk-rs `ComponentManifest` subset that concerns
//! the component backend process (ts mirror of slsdk-rs `src/manifest.rs` serde shapes).
//!
//! Field names keep their Rust snake_case form with no rename: the shell and slsdk-rs share
//! the serde contract, and this mirror keeps structural isomorphism so a manifest document
//! can be deserialized by either side with zero conversion (`Option` fields → optional ts
//! fields). Only the fields the server-side component carries are mirrored here; the
//! package-level fields (`menu` / `routes` / `files`) belong to the shell's installation
//! plane and are not consumed by the serve surface.

/** JSON value mirror of `serde_json::Value` (the manifest is a JSON document with zero conversion). */
export type JsonValue = string | number | boolean | null | JsonValue[] | { [key: string]: JsonValue };

/** The manifest the component backend receives (architecture.md "Installation and Lifecycle").
 *
 * `serve()` itself only reads `id` / `version` (the `/health` identity payload); the rest of
 * the declaration is carried as-is so a component can keep a single manifest source in memory.
 */
export interface ServerManifest {
  /** Component unique id: the install directory name and the `/comps/{id}` static URL key. */
  id: string;
  /** Component version. */
  version: string;
  /** Minimum compatible shell version (the shell gates on it at install time). */
  min_shell_version: string;
  /** Namespace registered at the shell gateway: the `ns` of `/api/{ns}/{rest}`. */
  namespace: string;
  /** Display name (used by menu / settings pages). */
  display_name: string;
  /** UI declaration; absent = pure-backend shape (no UI mount surface). */
  ui?: UiDeclaration;
  /** Backend launch: the command the shell spawns as a subprocess; absent = pure-frontend shape. */
  launch?: LaunchConfig;
  /** Config declaration (architecture.md "Configuration Plane"); absent = no configurable items. */
  config?: ComponentConfig;
  /** Declared transport capabilities of the backend process (the shell reads it when
   * choosing the injected `SL_ENDPOINT` descriptor); absent = every transport the host
   * platform offers (Unix domain socket on Linux/macOS, loopback TCP everywhere). */
  transport?: TransportCaps;
}

/** Transport capability tiers (architecture.md "Plane B contract · transport"; lowercase
 * serde rename): `"tcponly"` = the shell injects a random loopback `tcp:` descriptor on
 * every platform (a runtime with no Unix domain socket support declares its restriction
 * here instead of failing at bind time). */
export type TransportCaps = "tcponly";

/** UI mounting shape (architecture.md "Plane A Contract"): two UI container tiers. */
export interface UiDeclaration {
  /** `module`: React component dynamically imported into the host route tree. `iframe`: isolated iframe container. */
  ui_type: UiType;
  /** Relative path of the UI entry: an ESM bundle for the module tier, HTML for the iframe tier. */
  entry: string;
}

export type UiType = "module" | "iframe";

/** Backend launch config: the shell spawns an independent subprocess with `command + args`. */
export interface LaunchConfig {
  command: string;
  args: string[];
}

/** Config declaration section: field list + label i18n mapping. */
export interface ComponentConfig {
  /** Declared config field list (the rendering basis of the shell config page). */
  fields: ConfigField[];
  /** Label i18n: lang → static JSON file path relative to the component's `ui/` directory. */
  i18n?: Record<string, string>;
}

/** Declaration of a single config field (architecture.md "Configuration Plane · Config Declaration"). */
export interface ConfigField {
  /** TOML path-form key: dotted paths within segments are supported (e.g. `"tushare.token"`). */
  key: string;
  /** i18n key of the display label (resolved via {@link ComponentConfig.i18n}). */
  label: string;
  /** Value type tier (determines endpoint validation and config page control shape). */
  type: ConfigFieldType;
  /** Default value: the merge fallback when config.toml lacks the entry (GET semantics). */
  default?: JsonValue;
  /** true = must have a non-empty value in the merged state, otherwise the config gate blocks it. */
  required?: boolean;
  /** Enum value list for the select tier (should be absent for other types). */
  options?: string[];
  /** Format constraint: v1 recognizes `"cron"`; other values pass through unvalidated. */
  format?: string;
  /** true = takes effect only after the component restarts; false = runtime (hot-reloaded) item. */
  restart_required?: boolean;
  /** Sensitive value marker: masking is the shell UI rendering layer's responsibility. */
  sensitive?: boolean;
  /** Grouping hint (used by the shell config page for grouped rendering). */
  group?: string;
}

/** Config value types (architecture.md "Configuration Plane", five tiers; lowercase serde rename). */
export type ConfigFieldType = "string" | "number" | "boolean" | "select" | "datetime";
