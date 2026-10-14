// Vendor shim (GENERATED — do not hand-edit; regenerate with
// `node scripts/gen-vendor-shims.mjs` after a react/react-dom bump):
// the react-dom whitelist entry.
// Property-extraction form: vite's pre-bundled CJS-interop artifacts expose
// only a default export, so export * / static re-exports forward zero named
// bindings — cross-origin component bundles statically importing named
// bindings would fail module linking. The build path (esbuildDirectBundle,
// cjsShimSource) is unaffected. The accessor is cast: a few runtime-only
// keys have no @types counterparts.
import vendorImport from 'react-dom';

const vendor = vendorImport as unknown as Record<string, unknown>;

export default vendorImport;

export const
  __DOM_INTERNALS_DO_NOT_USE_OR_WARN_USERS_THEY_CANNOT_UPGRADE = vendor.__DOM_INTERNALS_DO_NOT_USE_OR_WARN_USERS_THEY_CANNOT_UPGRADE,
  browser = vendor.browser,
  createPortal = vendor.createPortal,
  flushSync = vendor.flushSync,
  preconnect = vendor.preconnect,
  prefetchDNS = vendor.prefetchDNS,
  preinit = vendor.preinit,
  preinitModule = vendor.preinitModule,
  preload = vendor.preload,
  preloadModule = vendor.preloadModule,
  requestFormReset = vendor.requestFormReset,
  unstable_batchedUpdates = vendor.unstable_batchedUpdates,
  useFormState = vendor.useFormState,
  useFormStatus = vendor.useFormStatus,
  version = vendor.version;
