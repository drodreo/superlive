// Shell frontend service layer: the single Observable-based source of truth
// for tauri commands and the shell's embedded service endpoints.
// All URL construction toward the 39876 endpoint (component APIs / static
// resources) derives from SHELL_ORIGIN;
// hand-joining addresses inside component code is forbidden.

import { invoke } from '@tauri-apps/api/core';
import { defer, from, type Observable } from 'rxjs';

// The shell's embedded HTTP service endpoint (consistent with src-tauri
// config::SHELL_PORT).
// The shell webview's origin is cross-origin to it: component ESM dynamic
// loading, iframes, and API fetches all use the full address, with cross-origin
// allowed by the shell service's CORS middleware.
export const SHELL_ORIGIN = 'http://127.0.0.1:39876';

// ---------- View types aligned with the Rust ComponentView (commands.rs) ----------
// Field names stay snake_case: consistent with the manifest contract fields
// (slsdk_rs's serde form).

export interface UiDeclaration {
  ui_type: 'module' | 'iframe';
  entry: string;
}

export interface MenuDeclaration {
  title: string;
  icon_path: string | null;
}

export interface RouteDeclaration {
  path: string;
  title: string;
}

export interface ComponentInfo {
  id: string;
  version: string;
  namespace: string;
  display_name: string;
  running: boolean;
  ui: UiDeclaration;
  menu: MenuDeclaration | null;
  routes: RouteDeclaration[];
}

// ---------- Shell-level token: a singleton fetched once at startup ----------

let tokenPromise: Promise<string> | null = null;

/** Shell-level token (the shell_token command). Cached after the first call; later calls reuse the same Promise. */
export function shellToken(): Promise<string> {
  tokenPromise ??= invoke<string>('shell_token');
  return tokenPromise;
}

// ---------- Observable wrappers over commands ----------
// Wrapped in defer: lazy evaluation + a fresh start per subscription (an rxjs
// convention: subscribing is executing).

function invoke$<T>(cmd: string, args?: Record<string, unknown>): Observable<T> {
  return defer(() => from(invoke<T>(cmd, args)));
}

/** Installed component list (on-disk registration + running state merged). */
export const listComponents = (): Observable<ComponentInfo[]> =>
  invoke$<ComponentInfo[]>('list_components');

/** Install a component package (zip disk path). */
export const installComponent = (zipPath: string): Observable<ComponentInfo> =>
  invoke$<ComponentInfo>('install_component', { zipPath });

/** Uninstall a component (stop the process first, then delete data). */
export const uninstallComponent = (id: string): Observable<void> =>
  invoke$<void>('uninstall_component', { id });

/** Start a component backend. */
export const startComponent = (id: string): Observable<void> =>
  invoke$<void>('start_component', { id });

/** Stop a component backend. */
export const stopComponent = (id: string): Observable<void> =>
  invoke$<void>('stop_component', { id });

// ---------- Shell endpoint URLs and /api requests ----------

/** Component UI static asset URL (/comps/{id}/{entry}; shared by module dynamic import and iframe). */
export function componentAssetUrl(id: string, entry: string): string {
  return `${SHELL_ORIGIN}/comps/${id}/${entry.replace(/^\/+/, '')}`;
}

/**
 * Component API call (the shell frontend's own /api channel): builds the full
 * address automatically and injects the Bearer token.
 * This is the in-shell equivalent of @shell/sdk — the body semantics are
 * exactly the same as native fetch (a raw byte stream).
 */
export async function shellApi(path: string, init?: RequestInit): Promise<Response> {
  const token = await shellToken();
  const headers = new Headers(init?.headers);
  headers.set('Authorization', `Bearer ${token}`);
  return fetch(`${SHELL_ORIGIN}/api/${path.replace(/^\/+/, '')}`, { ...init, headers });
}

// ---------- Configuration plane (architecture.md "Configuration plane" contract) ----------
// The command contract is provided by the shell's Rust line:
// get_component_config / write_component_config /
// get_component_state / set_launch_policy. Returned fields stay snake_case
// (consistent with the slsdk-rs manifest serde form).

/** A manifest config declaration entry (an element of the fields array returned by get_component_config). */
export interface ConfigField {
  key: string;
  /** i18n key or raw text (the config page looks it up in the i18n map, falling back to the raw text when missing). */
  label: string;
  type: 'string' | 'number' | 'boolean' | 'select' | 'datetime';
  default: unknown;
  required: boolean;
  /** Option enum for the select type. */
  options: string[] | null;
  /** Format constraint (e.g. cron); validation authority lives on the shell Rust side, not re-implemented in the UI. */
  format: string | null;
  restart_required: boolean;
  /** Sensitive value: rendered as a password field + visibility toggle. */
  sensitive: boolean;
  /** Group name (i18n key or raw text); null = ungrouped. */
  group: string | null;
}

export interface ComponentConfig {
  fields: ConfigField[];
  /** Current values map; keys with no declared default and no file value are omitted (shown as placeholders on the config page). */
  values: Record<string, string | number | boolean>;
  /** i18n resource path map (lang → relative path inside ui/), fetched via the /comps/{id}/ static space. */
  i18n: Record<string, string> | null;
}

/** The component lifecycle's four states (the shell-side state machine). */
export type ComponentState = 'needs-config' | 'ready' | 'running' | 'stopped';

/** Launch policy (component-level, persisted in the shell-side registry, not in the component manifest). */
export type LaunchPolicy = 'auto' | 'manual';

/** Component config declaration + current values (the config page's data source). */
export const getComponentConfig = (id: string): Observable<ComponentConfig> =>
  invoke$<ComponentConfig>('get_component_config', { id });

export interface WriteConfigResult {
  /** Whether this write changed a restart_required entry (a running component needs a restart to take effect). */
  restart_required_changed: boolean;
}

/**
 * Write the component config. On failure (shell-side validation rejected) the
 * command rejects with Err(JSON string), shaped {"errors":{key: reason}} —
 * the config page parses it and flags fields one by one.
 */
export const writeComponentConfig = (
  id: string,
  values: Record<string, string | number | boolean>,
): Observable<WriteConfigResult> => invoke$<WriteConfigResult>('write_component_config', { id, values });

/** The component's current lifecycle state. */
export const getComponentState = (id: string): Observable<ComponentState> =>
  invoke$<ComponentState>('get_component_state', { id });

/** Set the component's launch policy. */
export const setLaunchPolicy = (id: string, policy: LaunchPolicy): Observable<void> =>
  invoke$<void>('set_launch_policy', { id, policy });

/**
 * Config i18n resource loading: picks the path for the current language from
 * the i18n map and fetches the JSON from the component's ui static space
 * (/comps/{id}/). The config page does not depend on the component ESM
 * loading successfully; missing/corrupt resources silently return null
 * (label/group fall back to the raw text) without blocking the config page.
 */
export async function fetchConfigI18n(
  id: string,
  i18n: Record<string, string> | null,
): Promise<Record<string, string> | null> {
  if (!i18n) return null;
  const langs = Object.keys(i18n);
  if (langs.length === 0) return null;
  const nav = (navigator.language || '').toLowerCase();
  // Language selection: exact match → prefix match (declared zh ↔ browser zh-CN) → first declared entry as fallback
  const lang =
    langs.find((l) => l.toLowerCase() === nav) ??
    langs.find((l) => nav.startsWith(l.toLowerCase())) ??
    langs[0];
  try {
    const res = await fetch(componentAssetUrl(id, i18n[lang]));
    if (!res.ok) return null;
    return (await res.json()) as Record<string, string>;
  } catch {
    return null;
  }
}
