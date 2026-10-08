/**
 * useShellState - hook for shell-injection readiness (extracted from sl-finance App.tsx into the shared facility).
 *
 * Shell environment: window.__SL_SHELL__ = { token, origin } (injected by the shell App.tsx at startup).
 * Shell injection timing (architecture.md "UI-side token handoff"): the shell App kicks off an async
 * invoke at module top level to fetch the token and write window.__SL_SHELL__; "started before this component mounts" does not imply
 * "finished before mount" — there is a race window when a component route is restored directly on first launch. When missing, poll briefly
 * (200ms × 10 = 2s) and resume rendering automatically once ready; only a timeout returns 'missing', the waiting period returns
 * 'waiting' (components should render null to avoid flashing a fallback page).
 */
import { useEffect, useState } from 'react';

/** Shell-injected environment (aligned with the shell App.tsx injection shape) */
export interface ShellEnv {
  token: string;
  origin: string;
}

declare global {
  interface Window {
    __SL_SHELL__?: ShellEnv;
  }
}

/** Shell environment readiness: ready = usable; waiting = shell injection not finished yet (async); missing = wait timed out */
export type ShellState = 'ready' | 'waiting' | 'missing';

export function useShellState(): ShellState {
  const [shell, setShell] = useState<ShellState>(() =>
    window.__SL_SHELL__ ? 'ready' : 'waiting',
  );

  useEffect(() => {
    if (shell !== 'waiting') return;
    let tries = 0;
    const timer = window.setInterval(() => {
      tries += 1;
      if (window.__SL_SHELL__) {
        window.clearInterval(timer);
        setShell('ready');
      } else if (tries >= 10) {
        window.clearInterval(timer);
        setShell('missing');
      }
    }, 200);
    return () => window.clearInterval(timer);
  }, [shell]);

  return shell;
}
