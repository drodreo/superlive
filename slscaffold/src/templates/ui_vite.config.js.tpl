import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

// package.json is type: module, so vite loads this config as ESM — no __dirname.
const root = dirname(fileURLToPath(import.meta.url));

// Shared-library whitelist (the shell's Plane A contract): kept external at build
// time so the artifact preserves bare imports; at runtime the shell's import map
// resolves them to the shell vendor bundles — react / rxjs / i18next stay app-wide
// singletons (a second copy of react crashes with "Invalid hook call").
// '@shell/sdk' (api entry) and '@shell/sdk/ui' (ui entry) are import-map keys too:
// source imports use these official facade names, and resolve.alias maps the prefix
// to the local slsdk-ts dependency for type + build-time resolution (its
// package.json exports map './ui' accordingly). react-dom/client and the
// jsx-runtime subpaths are internal subpath mappings of whitelisted packages, not
// additions to the whitelist.
const SHARED_IMPORTS = [
  'react',
  'react/jsx-runtime',
  'react/jsx-dev-runtime',
  'react-dom',
  'react-dom/client',
  'rxjs',
  'i18next',
  'react-i18next',
  '@shell/sdk',
  '@shell/sdk/ui',
];

export default defineConfig({
  // VITE_BASE: set only if a standalone deployment sits behind a path prefix;
  // the component artifact's entry.js uses relative imports and is unaffected.
  base: process.env.VITE_BASE || '/',
  plugins: [react()],
  resolve: {
    alias: {
      '@shell/sdk': 'slsdk-ts',
    },
  },
  server: {
    proxy: {
      // Standalone dev: /api goes straight to the local backend (start it with
      // SL_ENDPOINT=tcp:127.0.0.1:__DEV_PORT__ — see README.md).
      '/api': {
        target: 'http://localhost:__DEV_PORT__',
        changeOrigin: true,
      },
    },
  },
  build: {
    rollupOptions: {
      // Single build entry: the component ESM (the shell's module tier dynamically
      // imports it; the default export is the component root). index.html and
      // main.tsx only serve vite dev and never enter the build output.
      input: resolve(root, 'src/entry.tsx'),
      // The entry signature must be preserved: the consumer (the shell's
      // ComponentHost) is a runtime dynamic import, invisible to rollup at build
      // time — without this, the default-exported component tree gets tree-shaken
      // away entirely and the artifact becomes an empty shell (no component code,
      // no default export — verified the hard way).
      preserveEntrySignatures: 'strict',
      external: (source) => SHARED_IMPORTS.includes(source),
      output: {
        // Fixed file names (no hash): manifest.ui.entry statically points at entry.js.
        entryFileNames: 'entry.js',
        chunkFileNames: 'assets/chunk-[hash].js',
        assetFileNames: 'assets/[name]-[hash][extname]',
      },
    },
  },
});
