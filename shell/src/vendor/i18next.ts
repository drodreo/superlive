// Vendor shim: the i18next whitelist entry (the shared i18n instance — the
// shell's i18n init, the sdk-ui registerI18n helper and module-tier
// components all resolve the same singleton through the import map; see
// WHITELIST_IMPORTS). The build artifact is bundled directly by esbuild;
// this shim serves the dev path only (the vendorDevServer compiles it on
// the fly; dev has no tree-shaking).
export * from 'i18next';
export { default } from 'i18next';
