// Shell app framework: sidebar menu + content area.
// The menu = dynamic entries (manifest.menu registrations of on-disk
// registered components) + a bottom block pinned to the sidebar foot
// (component management + settings entries, app version line); route
// state comes from route$ (hash routing), and the content area dispatches
// to the welcome page / management page / settings page / component
// config page / component container by route.

import { useEffect, useState } from 'react';
import { getVersion } from '@tauri-apps/api/app';
import { useTranslation } from 'react-i18next';
import { listComponents, SHELL_ORIGIN, shellToken, type ComponentInfo } from './api';
import { navigate, route$, type Route } from './router';
import { componentMenuTitle, injectMenuTitles } from './i18n';
import { ComponentConfigPage } from './ComponentConfigPage';
import { ComponentHost } from './ComponentHost';
import { ComponentManager } from './ComponentManager';
import { SettingsPage } from './SettingsPage';
import { WelcomePage } from './WelcomePage';
import { ActivityBar, type Theme } from './ActivityBar';
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

// Theme bootstrap: stamp the persisted theme on <html> at module eval —
// ahead of React's first render, so dark-theme users get no light-mode
// flash on launch.
const initialTheme: Theme = localStorage.getItem('sl-theme') === 'dark' ? 'dark' : 'light';
document.documentElement.dataset.theme = initialTheme;

export default function App() {
  const { t } = useTranslation();
  const [route, setRoute] = useState<Route>(route$.value);
  const [components, setComponents] = useState<ComponentInfo[]>([]);
  // Component list refresh counter: bumped after the management page
  // installs/uninstalls, triggering a list re-fetch (the dynamic menu updates with it)
  const [listVersion, setListVersion] = useState(0);
  // App version for the sidebar foot — the config omits `version`, so this
  // is the Cargo.toml package version (the single version source)
  const [version, setVersion] = useState('');
  // App theme (light/dark); bootstrapped pre-render from localStorage
  const [theme, setTheme] = useState<Theme>(initialTheme);

  // Route state subscription (a BehaviorSubject yields the current value on subscribe)
  useEffect(() => {
    const sub = route$.subscribe(setRoute);
    return () => sub.unsubscribe();
  }, []);

  // App version (best-effort display; failure leaves the placeholder)
  useEffect(() => {
    let alive = true;
    getVersion()
      .then((v) => {
        if (alive) setVersion(v);
      })
      .catch(() => {});
    return () => {
      alive = false;
    };
  }, []);

  // Theme changes: <html data-theme> drives the palette; persist the choice
  useEffect(() => {
    document.documentElement.dataset.theme = theme;
    localStorage.setItem('sl-theme', theme);
  }, [theme]);

  // Component list: the dynamic menu's data source + the component page's mount info source
  useEffect(() => {
    const sub = listComponents().subscribe({
      next: setComponents,
      error: (e) => console.error('[App] failed to read component list:', e),
    });
    return () => sub.unsubscribe();
  }, [route, listVersion]);

  // Menu-title resource injection (architecture.md "Localized titles"): the
  // manifest's localized titles land in each component's own namespace under
  // the fixed `menuTitle` key (zh/en seeded with the manifest `title` as the
  // terminal fallback). Runs after every list fetch; injection is idempotent
  // and registered namespaces stay for the session. react-i18next re-renders
  // subscribers when bundles land, so the first paint may briefly show the
  // single-language `title` fallback before the localized value arrives.
  useEffect(() => {
    for (const c of components) {
      if (!c.menu) continue;
      injectMenuTitles(c.namespace, c.menu.titles ?? {}, c.menu.title);
    }
  }, [components]);

  // Dynamic menu entries: components with registered menus (including
  // non-running ones — the menu still shows; clicking yields feedback from the container)
  const menuComponents = components.filter((c) => c.menu);
  const activeComponent =
    route.name === 'component' ? components.find((c) => c.namespace === route.ns) : undefined;

  // Localized menu title of a component (null = no menu registration):
  // the menuTitle chain with the single-language `title` as the local fallback
  const menuTitleOf = (c: ComponentInfo | undefined): string | null =>
    c?.menu ? (componentMenuTitle(c.namespace) ?? c.menu.title) : null;

  // Activity-bar module title: the active context's title; the neutral
  // landing (welcome) falls back to the app name
  const moduleTitle =
    route.name === 'components'
      ? t('menu.components')
      : route.name === 'settings'
        ? t('menu.settings')
        : route.name === 'config'
          ? (menuTitleOf(components.find((c) => c.id === route.id)) ?? t('title.configFallback'))
          : route.name === 'component'
            ? (menuTitleOf(activeComponent) ?? route.ns)
            : 'Superlive';

  return (
    <div className="app">
      <aside className="sidebar">
        <div className="brand">Superlive</div>
        <nav className="menu">
          {menuComponents.map((c) => {
            const title = menuTitleOf(c) ?? c.menu!.title;
            return (
              <button
                type="button"
                key={c.id}
                className={route.name === 'component' && route.ns === c.namespace ? 'active' : ''}
                title={title}
                onClick={() =>
                  navigate(`/comp/${c.namespace}/${c.routes[0]?.path.replace(/^\/+/, '') ?? ''}`)
                }
              >
                {title}
              </button>
            );
          })}
          <div className="menu-foot">
            <button
              type="button"
              className={route.name === 'components' ? 'active' : ''}
              onClick={() => navigate('/components')}
            >
              {t('menu.components')}
            </button>
            <button
              type="button"
              className={route.name === 'settings' ? 'active' : ''}
              onClick={() => navigate('/settings')}
            >
              {t('menu.settings')}
            </button>
            <div className="menu-version">v{version || '…'}</div>
          </div>
        </nav>
      </aside>

      <div className="workspace">
        <ActivityBar
          title={moduleTitle}
          theme={theme}
          onToggleTheme={() => setTheme((t) => (t === 'light' ? 'dark' : 'light'))}
        />

        <main className="content">
          {route.name === 'home' && <WelcomePage />}

          {route.name === 'components' && (
            <ComponentManager onChanged={() => setListVersion((v) => v + 1)} />
          )}

          {route.name === 'settings' && <SettingsPage />}

          {route.name === 'config' && <ComponentConfigPage id={route.id} />}

          {route.name === 'component' &&
            (activeComponent ? (
              <ComponentHost info={activeComponent} route={route.path} />
            ) : (
              <div className="comp-error">{t('app.compNotFound', { ns: route.ns })}</div>
            ))}
        </main>
      </div>
    </div>
  );
}
