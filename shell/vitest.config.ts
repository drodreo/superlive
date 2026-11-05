// Vitest configuration for the shell test suite (SurfaceHost and friends).
//
// Kept as a separate file on purpose: vite.config.ts encodes the dev/build
// vendor-import-map machinery and must not grow a test branch. Vitest picks
// this file up automatically and vite build never reads it.
//
// Alias notes (the test-mode counterpart of the dev/build alias story in
// vite.config.ts):
// - `@shell/sdk/ui` loads the slsdk-ts source directly (build keeps the bare
//   import external to the import map, dev serves the esbuild bundle under
//   /vendor — tests need their own resolution).
// - react / react-dom / rxjs / i18next / react-i18next are pinned to the
//   shell's own node_modules copies: the slsdk-ts sources would otherwise
//   resolve their bare imports through slsdk-ts/node_modules, producing a
//   second react instance (the dual-instance hooks crash the import-map
//   whitelist decision guards against).
import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = dirname(fileURLToPath(import.meta.url));

export default defineConfig({
  plugins: [react()],
  resolve: {
    alias: [
      {
        find: "@shell/sdk/ui",
        replacement: resolve(root, "../slsdk-ts/src/ui/index.ts"),
      },
      {
        find: "react-dom",
        replacement: resolve(root, "node_modules/react-dom"),
      },
      { find: "react", replacement: resolve(root, "node_modules/react") },
      { find: "rxjs", replacement: resolve(root, "node_modules/rxjs") },
      { find: "i18next", replacement: resolve(root, "node_modules/i18next") },
      {
        find: "react-i18next",
        replacement: resolve(root, "node_modules/react-i18next"),
      },
    ],
  },
  test: {
    environment: "jsdom",
    setupFiles: ["./src/test/setup.ts"],
  },
});
