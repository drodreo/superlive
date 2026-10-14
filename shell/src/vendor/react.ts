// Vendor shim (GENERATED — do not hand-edit; regenerate with
// `node scripts/gen-vendor-shims.mjs` after a react/react-dom bump):
// the react whitelist entry.
// Property-extraction form: vite's pre-bundled CJS-interop artifacts expose
// only a default export, so export * / static re-exports forward zero named
// bindings — cross-origin component bundles statically importing named
// bindings would fail module linking. The build path (esbuildDirectBundle,
// cjsShimSource) is unaffected. The accessor is cast: a few runtime-only
// keys have no @types counterparts.
import vendorImport from 'react';

const vendor = vendorImport as unknown as Record<string, unknown>;

export default vendorImport;

export const
  Activity = vendor.Activity,
  Children = vendor.Children,
  Component = vendor.Component,
  Fragment = vendor.Fragment,
  Profiler = vendor.Profiler,
  PureComponent = vendor.PureComponent,
  StrictMode = vendor.StrictMode,
  Suspense = vendor.Suspense,
  ViewTransition = vendor.ViewTransition,
  __CLIENT_INTERNALS_DO_NOT_USE_OR_WARN_USERS_THEY_CANNOT_UPGRADE = vendor.__CLIENT_INTERNALS_DO_NOT_USE_OR_WARN_USERS_THEY_CANNOT_UPGRADE,
  __COMPILER_RUNTIME = vendor.__COMPILER_RUNTIME,
  act = vendor.act,
  addTransitionType = vendor.addTransitionType,
  cache = vendor.cache,
  cacheSignal = vendor.cacheSignal,
  captureOwnerStack = vendor.captureOwnerStack,
  cloneElement = vendor.cloneElement,
  createContext = vendor.createContext,
  createElement = vendor.createElement,
  createRef = vendor.createRef,
  forwardRef = vendor.forwardRef,
  isValidElement = vendor.isValidElement,
  lazy = vendor.lazy,
  memo = vendor.memo,
  startTransition = vendor.startTransition,
  unstable_useCacheRefresh = vendor.unstable_useCacheRefresh,
  use = vendor.use,
  useActionState = vendor.useActionState,
  useCallback = vendor.useCallback,
  useContext = vendor.useContext,
  useDebugValue = vendor.useDebugValue,
  useDeferredValue = vendor.useDeferredValue,
  useEffect = vendor.useEffect,
  useEffectEvent = vendor.useEffectEvent,
  useId = vendor.useId,
  useImperativeHandle = vendor.useImperativeHandle,
  useInsertionEffect = vendor.useInsertionEffect,
  useLayoutEffect = vendor.useLayoutEffect,
  useMemo = vendor.useMemo,
  useOptimistic = vendor.useOptimistic,
  useReducer = vendor.useReducer,
  useRef = vendor.useRef,
  useState = vendor.useState,
  useSyncExternalStore = vendor.useSyncExternalStore,
  useTransition = vendor.useTransition,
  version = vendor.version;
