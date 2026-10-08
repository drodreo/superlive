/**
 * Style injection/unloading mechanism for the shared UI facility.
 *
 * Mirrors the componentization-era ?inline semantics: the style text (css.ts) travels with the bundle,
 * a <style> node is injected into head on mount and removed on unmount — styles follow the component lifecycle
 * in and out of the host document, never resident (zero residue after the shell unmounts the component).
 *
 * Reference counting: components consuming the ui facility simultaneously share one <style> node;
 * it is removed only when the last consumer unmounts; React StrictMode's mount→cleanup→mount sequence is idempotent.
 */
import { useEffect } from 'react';
import { uiCssText } from './css';

const STYLE_ID = 'sl-ui-shared-styles';

let refCount = 0;

/** Injects the shared facility styles (idempotent: the first consumer creates the node, later ones just count) */
export function injectUiCss(): void {
  refCount += 1;
  if (refCount > 1) return;
  if (document.getElementById(STYLE_ID)) {
    // The host already has a node with the same id (leftover from an abnormal path, e.g. hot reload) — reuse it instead of recreating
    return;
  }
  const style = document.createElement('style');
  style.id = STYLE_ID;
  style.textContent = uiCssText;
  document.head.appendChild(style);
}

/** Unloads the shared facility styles (pairs with injectUiCss; the node is removed only when the count reaches zero) */
export function removeUiCss(): void {
  refCount = Math.max(0, refCount - 1);
  if (refCount > 0) return;
  document.getElementById(STYLE_ID)?.remove();
}

/** React convenience hook: one-line adoption via useEffect (inject on mount, remove on unmount) */
export function useUiCss(): void {
  useEffect(() => {
    injectUiCss();
    return () => removeUiCss();
  }, []);
}
