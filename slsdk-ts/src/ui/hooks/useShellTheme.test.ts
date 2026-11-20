import { act, renderHook } from "@testing-library/react";
import { BehaviorSubject } from "rxjs";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ShellEnv } from "./useShellState";
import { useShellTheme, type Theme } from "./useShellTheme";

function setShell(env: Partial<ShellEnv> | undefined): void {
  (window as unknown as { __SL_SHELL__?: Partial<ShellEnv> }).__SL_SHELL__ =
    env;
}

function shellWithStream(
  value: Theme,
): ShellEnv & { theme$: BehaviorSubject<Theme> } {
  return {
    token: "t",
    origin: "http://localhost",
    theme: value,
    theme$: new BehaviorSubject<Theme>(value),
  };
}

describe("useShellTheme", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
    setShell(undefined);
  });

  it("returns the current stream value when the shell is already injected, and follows toggles", () => {
    const env = shellWithStream("light");
    setShell(env);
    const { result } = renderHook(() => useShellTheme());
    // BehaviorSubject: subscribing yields the current value.
    expect(result.current).toBe("light");

    act(() => env.theme$.next("dark"));
    expect(result.current).toBe("dark");

    act(() => env.theme$.next("light"));
    expect(result.current).toBe("light");
  });

  it("waits with null during the injection race, then corrects from the stream", () => {
    const { result } = renderHook(() => useShellTheme());
    // Shell not injected yet: the waiting discipline is null (same as useShellState).
    expect(result.current).toBeNull();

    // Mid-poll injection (inside the 200ms × 10 window): tick 1 sees nothing…
    act(() => vi.advanceTimersByTime(200));
    expect(result.current).toBeNull();

    // …the environment lands, tick 2 picks it up and the stream corrects the value.
    const env = shellWithStream("dark");
    setShell(env);
    act(() => vi.advanceTimersByTime(200));
    expect(result.current).toBe("dark");

    // Subsequent toggles keep following.
    act(() => env.theme$.next("light"));
    expect(result.current).toBe("light");
  });

  it("keeps null when the shell never injects (2s timeout)", () => {
    const { result } = renderHook(() => useShellTheme());
    act(() => vi.advanceTimersByTime(2000));
    expect(result.current).toBeNull();
  });

  it("falls back to the static snapshot when the shell carries no theme$ stream", () => {
    setShell({ token: "t", origin: "http://localhost", theme: "dark" });
    const { result } = renderHook(() => useShellTheme());
    expect(result.current).toBe("dark");

    // Non-reactive fallback: the value stays the snapshot, no crash, no polling churn.
    act(() => vi.advanceTimersByTime(2000));
    expect(result.current).toBe("dark");
  });
});
