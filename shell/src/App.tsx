// Shell app framework: sidebar menu + content area.
// The menu = fixed entries (component management) + dynamic entries
// (manifest.menu registrations of on-disk registered components);
// route state comes from route$ (hash routing), and the content area
// dispatches to the management page / component config page / component
// container by route.

import { useEffect, useState } from 'react';
import { listComponents, SHELL_ORIGIN, shellToken, type ComponentInfo } from './api';
import { navigate, route$, type Route } from './router';
import { ComponentConfigPage } from './ComponentConfigPage';
import { ComponentHost } from './ComponentHost';
import { ComponentManager } from './ComponentManager';
import './App.css';

// Shell environment injection: component UIs read { token, origin } from
// window.__SL_SHELL__ to make /api calls. Placed at module top level —
// evaluation fires it, before any component mounts or the first fetch;
// an idempotent write, since the token is the shell-level singleton.
void shellToken().then((token) => {
  (window as unknown as { __SL_SHELL__?: { token: string; origin: string } }).__SL_SHELL__ = {
    token,
    origin: SHELL_ORIGIN,
  };
});

export default function App() {
  const [route, setRoute] = useState<Route>(route$.value);
  const [components, setComponents] = useState<ComponentInfo[]>([]);
  // Component list refresh counter: bumped after the management page
  // installs/uninstalls, triggering a list re-fetch (the dynamic menu updates with it)
  const [listVersion, setListVersion] = useState(0);

  // Route state subscription (a BehaviorSubject yields the current value on subscribe)
  useEffect(() => {
    const sub = route$.subscribe(setRoute);
    return () => sub.unsubscribe();
  }, []);

  // Component list: the dynamic menu's data source + the component page's mount info source
  useEffect(() => {
    const sub = listComponents().subscribe({
      next: setComponents,
      error: (e) => console.error('[App] 读取组件列表失败:', e),
    });
    return () => sub.unsubscribe();
  }, [route, listVersion]);

  // Dynamic menu entries: components with registered menus (including
  // non-running ones — the menu still shows; clicking yields feedback from the container)
  const menuComponents = components.filter((c) => c.menu);
  const activeComponent =
    route.name === 'component' ? components.find((c) => c.namespace === route.ns) : undefined;

  return (
    <div className="app">
      <aside className="sidebar">
        <div className="brand">superlive</div>
        <nav className="menu">
          <button
            type="button"
            className={route.name === 'components' ? 'active' : ''}
            onClick={() => navigate('/components')}
          >
            组件管理
          </button>
          {menuComponents.map((c) => (
            <button
              type="button"
              key={c.id}
              className={route.name === 'component' && route.ns === c.namespace ? 'active' : ''}
              title={c.menu!.title}
              onClick={() =>
                navigate(`/comp/${c.namespace}/${c.routes[0]?.path.replace(/^\/+/, '') ?? ''}`)
              }
            >
              {c.menu!.title}
            </button>
          ))}
        </nav>
      </aside>

      <main className="content">
        {route.name === 'components' && (
          <ComponentManager onChanged={() => setListVersion((v) => v + 1)} />
        )}

        {route.name === 'config' && <ComponentConfigPage id={route.id} />}

        {route.name === 'component' &&
          (activeComponent ? (
            <ComponentHost info={activeComponent} route={route.path} />
          ) : (
            <div className="comp-error">
              未找到命名空间为「{route.ns}」的已安装组件（可能已卸载，组件列表加载中或读取失败）
            </div>
          ))}
      </main>
    </div>
  );
}
