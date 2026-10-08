// Lightweight hash router: an rxjs BehaviorSubject holds the current route
// state (no react-router, zero new dependencies). Route forms:
// #/components (component management), #/config/{id} (component config page,
// the configuration-plane start gate), and #/comp/{ns}/{path} (component page).

import { BehaviorSubject } from 'rxjs';

export type Route =
  | { name: 'components' }
  | { name: 'config'; id: string }
  | { name: 'component'; ns: string; path: string };

function parseHash(): Route {
  const hash = window.location.hash.replace(/^#/, '');
  // #/config/{id}: component config page (id is constrained by
  // manifest.validate to a safe character set)
  const configMatch = hash.match(/^\/config\/([^/]+)$/);
  if (configMatch) {
    return { name: 'config', id: configMatch[1] };
  }
  // #/comp/{ns}/{path...}: ns is constrained by manifest.validate to
  // letters/digits/hyphens
  const match = hash.match(/^\/comp\/([^/]+)(?:\/(.*))?$/);
  if (match) {
    return { name: 'component', ns: match[1], path: match[2] ?? '' };
  }
  return { name: 'components' };
}

/** Current route state stream: subscribing yields the current value; advanced on hashchange. */
export const route$ = new BehaviorSubject<Route>(parseHash());

window.addEventListener('hashchange', () => {
  route$.next(parseHash());
});

/** Navigate to the given hash path (e.g. '/components', '/comp/todo/items'). */
export function navigate(to: string): void {
  window.location.hash = to;
}
