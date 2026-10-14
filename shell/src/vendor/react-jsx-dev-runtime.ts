// Vendor shim (GENERATED — do not hand-edit; regenerate with
// `node scripts/gen-vendor-shims.mjs` after a react/react-dom bump):
// the react/jsx-dev-runtime subpath (dev jsx transform runtime).
// Property-extraction form: vite's pre-bundled CJS-interop artifacts expose
// only a default export, so export * / static re-exports forward zero named
// bindings — cross-origin component bundles statically importing named
// bindings would fail module linking. The build path (esbuildDirectBundle,
// cjsShimSource) is unaffected. The accessor is cast: a few runtime-only
// keys have no @types counterparts.
import vendorImport from 'react/jsx-dev-runtime';

const vendor = vendorImport as unknown as Record<string, unknown>;

export default vendorImport;

export const
  Fragment = vendor.Fragment,
  jsxDEV = vendor.jsxDEV;
