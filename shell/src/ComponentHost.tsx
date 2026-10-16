// UI container: two-tier mounting by manifest.ui.ui_type (architecture.md
// "Plane A contract").
// - module tier: dynamically imports the component ESM (a React default
//   export), mounted into the shell component tree via React.lazy — the
//   experience matches a monolith. Whitelisted-library imports inside the
//   component bundle are resolved by index.html's import map to the shell's
//   vendor artifacts, so the react/rxjs instances are unique app-wide.
// - iframe tier: the shell's static path mounted directly in an iframe, for
//   components with heterogeneous stacks or isolation needs.
// module-tier load and render failures are caught by the error boundary and
// do not take the shell down.

import React, { lazy, useMemo, Suspense } from "react";
import type { ComponentType } from "react";
import { useTranslation } from "react-i18next";
import { componentAssetUrl, type ComponentInfo } from "./api";
import i18n from "./i18n";

/** The minimal props contract a module-tier component receives: the shell route path passed through verbatim, consumed at the component's own discretion. */
export interface ComponentModuleProps {
  route: string;
}

type ModuleComponent = ComponentType<ComponentModuleProps>;

/** Boundary for component load/render failures: shows the error; the rest of the shell is unaffected. */
class ComponentErrorBoundary extends React.Component<
  { children: React.ReactNode },
  { error: Error | null }
> {
  state = { error: null as Error | null };

  static getDerivedStateFromError(error: Error) {
    return { error };
  }

  componentDidCatch(error: Error) {
    console.error("[ComponentHost] component render failed:", error);
  }

  render() {
    if (this.state.error) {
      // Direct instance lookup: a class boundary can't use the hook — the
      // parent re-renders on locale change (useTranslation), which re-renders
      // this boundary and retranslates
      return (
        <div className="comp-error">
          {i18n.t("host.renderFailed", { error: this.state.error.message })}
        </div>
      );
    }
    return this.props.children;
  }
}

export function ComponentHost({
  info,
  route,
}: {
  info: ComponentInfo;
  route: string;
}) {
  const { t } = useTranslation();
  // module tier: dynamically imports the component ESM at runtime.
  // @vite-ignore declares that the URL is only determined at runtime, so
  // vite performs no build-time analysis (preserving native dynamic import
  // semantics).
  // useMemo: the same component (id+entry) reuses the same lazy wrapper so
  // React does not unmount and remount it over a component-type identity
  // change. The lazy factory runs lazily (importing only on first render),
  // so creating it early for the iframe tier has no side effects — hooks
  // must be called before any early-return branch to keep call order stable.
  const LazyModule = useMemo(
    () =>
      lazy(async () => {
        // ui=None (pure-backend shape) has no entry: the factory can only be
        // reached through the null guard below, which renders nothing — the
        // entry check is a defensive backstop, not a reachable path
        const entry = info.ui?.entry;
        if (!entry) {
          throw new Error(`Component has no UI declaration (${info.id})`);
        }
        const mod = (await import(
          /* @vite-ignore */ componentAssetUrl(info.id, entry)
        )) as { default: ModuleComponent };
        if (!mod?.default) {
          throw new Error(
            `Component entry is missing a default export (${info.id}/${entry})`,
          );
        }
        return { default: mod.default };
      }),
    [info.id, info.ui],
  );

  // ui=None (pure-backend shape): no UI mount point — render nothing. Such
  // components carry no menu/routes by validate contract, so this branch is
  // only reachable by manual URL navigation.
  if (!info.ui) {
    return null;
  }

  // iframe tier: the static entry mounted directly. src is the full address
  // (the shell webview origin is cross-origin to 39876).
  if (info.ui.ui_type === "iframe") {
    return (
      <iframe
        className="comp-frame"
        src={componentAssetUrl(info.id, info.ui.entry)}
        title={info.display_name}
      />
    );
  }

  return (
    <ComponentErrorBoundary>
      <Suspense
        fallback={<div className="comp-loading">{t("host.loading")}</div>}
      >
        <LazyModule route={route} />
      </Suspense>
    </ComponentErrorBoundary>
  );
}
