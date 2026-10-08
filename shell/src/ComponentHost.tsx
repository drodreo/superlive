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

import React, { lazy, useMemo, Suspense } from 'react';
import type { ComponentType } from 'react';
import { componentAssetUrl, type ComponentInfo } from './api';

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
    console.error('[ComponentHost] 组件渲染失败:', error);
  }

  render() {
    if (this.state.error) {
      return (
        <div className="comp-error">
          组件加载或渲染失败：{this.state.error.message}
        </div>
      );
    }
    return this.props.children;
  }
}

export function ComponentHost({ info, route }: { info: ComponentInfo; route: string }) {
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
        const mod = (await import(
          /* @vite-ignore */ componentAssetUrl(info.id, info.ui.entry)
        )) as { default: ModuleComponent };
        if (!mod?.default) {
          throw new Error(`组件入口缺少默认导出（${info.id}/${info.ui.entry}）`);
        }
        return { default: mod.default };
      }),
    [info.id, info.ui.entry],
  );

  // iframe tier: the static entry mounted directly. src is the full address
  // (the shell webview origin is cross-origin to 39876).
  if (info.ui.ui_type === 'iframe') {
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
      <Suspense fallback={<div className="comp-loading">组件加载中…</div>}>
        <LazyModule route={route} />
      </Suspense>
    </ComponentErrorBoundary>
  );
}
