/**
 * useShellTheme - hook for the shell's current theme (light/dark), the single
 * source of truth for component UIs. Components always run inside the shell
 * (no out-of-shell form), so the shell theme is the only baseline — component
 * surfaces must follow it instead of @media (prefers-color-scheme).
 *
 * Shell environment: window.__SL_SHELL__.theme$ is an rxjs BehaviorSubject
 * carrying the current Theme (injected by the shell App.tsx alongside the
 * token). Subscribing yields the current value immediately; the shell pushes
 * the new value on every toggle, so subscribers re-render in lockstep with
 * the shell's own palette switch.
 *
 * Race handling mirrors useShellState: __SL_SHELL__ is written from an async
 * invoke callback at the shell's module top level, so a component mounted
 * early may find it absent. While absent, poll briefly (200ms × 10 = 2s);
 * the waiting period returns null (components should render null — the same
 * discipline as useShellState — never guess a theme), and once the shell is
 * ready the BehaviorSubject corrects the value (subscribing yields the
 * current value, no toggle needed). Only a timeout keeps null. A degraded
 * shell that carries the static `theme` snapshot but no theme$ stream gets
 * the snapshot as a non-reactive fallback.
 */
import { useEffect, useState } from "react";

/** Shell theme baseline, aligned with the shell's Theme union ("light" | "dark"). */
export type Theme = "light" | "dark";

/** Read the current theme from a ready shell environment, or null while absent. */
function readShellTheme(): Theme | null {
  const env = window.__SL_SHELL__;
  if (!env) return null;
  // Live stream first (its current value is authoritative); the static
  // snapshot as the degraded fallback; neither → null (the caller stays
  // waiting/missing).
  return env.theme$ ? env.theme$.value : (env.theme ?? null);
}

export function useShellTheme(): Theme | null {
  const [theme, setTheme] = useState<Theme | null>(readShellTheme);
  // Shell environment readiness: gates the poll loop (useShellState semantics).
  const [ready, setReady] = useState(() => !!window.__SL_SHELL__);

  // Race window: __SL_SHELL__ is injected asynchronously — poll briefly and
  // resume automatically once ready; a timeout leaves theme null (missing).
  useEffect(() => {
    if (ready) return;
    let tries = 0;
    const timer = window.setInterval(() => {
      tries += 1;
      if (window.__SL_SHELL__) {
        window.clearInterval(timer);
        setReady(true);
      } else if (tries >= 10) {
        window.clearInterval(timer);
      }
    }, 200);
    return () => window.clearInterval(timer);
  }, [ready]);

  // Live subscription: on readiness, follow the stream (a BehaviorSubject
  // yields the current value on subscribe — corrects the waiting-period null
  // immediately, even before the first toggle arrives).
  useEffect(() => {
    const stream = window.__SL_SHELL__?.theme$;
    if (!ready || !stream) return;
    const sub = stream.subscribe(setTheme);
    return () => sub.unsubscribe();
  }, [ready]);

  return theme;
}
