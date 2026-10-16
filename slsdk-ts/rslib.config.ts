import { defineConfig } from "@rslib/core";

export default defineConfig({
  source: {
    entry: {
      // api entry: HTTP convenience layer (five verbs + observe + body adaptation + ApiError) + dgpqp query primitives
      index: "./src/index.ts",
      // ui entry: shared UI facility (@shell/sdk/ui, see the "@shell/sdk dual entry" section in architecture.md)
      ui: "./src/ui/index.ts",
      // server entry: Plane B one-liner serve() for Node-family runtimes (node/deno/bun; node:http adapter + runtime dispatch)
      server: "./src/server/index.ts",
    },
  },
  lib: [
    {
      format: "esm",
      dts: true,
    },
  ],
  output: {
    distPath: {
      root: "dist",
    },
    externals: {
      // Whitelisted shared libs are provided as singletons by the host (the shell webview import map), not inlined at build time:
      // rxjs is a peerDependency of the api entry's observe; react/react-dom are peerDependencies of the ui entry
      // component facility (prevents duplicate React/rxjs instances; aligns with the shell whitelist);
      // i18next is externalized for the ui entry's registerI18n — the shared i18next instance
      // (the component i18n contract: components register namespaces into the host's instance)
      rxjs: "rxjs",
      react: "react",
      "react-dom": "react-dom",
      i18next: "i18next",
    },
  },
});
