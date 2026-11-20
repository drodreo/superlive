// Shell theme baseline: an rxjs BehaviorSubject holds the current theme —
// the same shape as route$ in router.ts (module-top singleton; subscribing
// yields the current value). The shell's own rendering keeps its useState
// path: App.tsx pushes here on every toggle and mirrors the value into its
// local state. Components consume the same stream through
// window.__SL_SHELL__.theme$ (injected by App.tsx) via @shell/sdk/ui's
// useShellTheme — the shell theme is the single baseline for component UIs.
// The initial value reads the persisted choice (localStorage sl-theme),
// matching the pre-render bootstrap that stamps <html data-theme>.
import { BehaviorSubject } from "rxjs";
import type { Theme } from "./ActivityBar";

/** Current theme stream: subscribing yields the current value; pushed on every toggle. */
export const theme$ = new BehaviorSubject<Theme>(
  localStorage.getItem("sl-theme") === "dark" ? "dark" : "light",
);
