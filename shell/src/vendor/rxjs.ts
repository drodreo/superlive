// Vendor shim: the rxjs whitelist entry (slsdk-ts's observe depends on rxjs;
// dual rxjs instances would break interop — the architecture.md "Plane A
// contract" whitelist decision).
// The build artifact is bundled directly by esbuild (vite rxDirectBundle;
// rollup would tree-shake the export * shim empty); this shim serves the dev
// path only (the vendorDevServer compiles it on the fly; dev has no
// tree-shaking).
export * from 'rxjs';
