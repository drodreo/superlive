// Vendor shim: the react-i18next whitelist entry (useTranslation etc.,
// bound to the shared i18next instance). Its internal react import stays
// bare — the dev path rewrites it to the same pre-bundled dependency the
// shell and components use (react singleton holds); the build artifact
// keeps it external for the import map. Dev path only, like rxjs.ts.
export * from 'react-i18next';
