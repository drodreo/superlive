import { defineConfig, type Plugin } from 'vite';
import react from '@vitejs/plugin-react';
import { createRequire } from 'node:module';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

// package.json is type: module, so the vite config loads as ESM — no __dirname
const root = dirname(fileURLToPath(import.meta.url));

// Vendor shims for the shared-library whitelist (architecture.md "Plane A
// contract" decisions 4/5):
// the react/react-dom shims work with index.html's import map to lock
// singletons; @shell/sdk is not on this list — it is a build artifact of
// slsdk-ts, statically copied at public/vendor/slsdk.js (its only external
// dependency rxjs happens to go through the import map too, naturally a
// singleton). react-dom/client and react/jsx-runtime(-dev-runtime) are
// internal subpath mappings of whitelisted packages (required for component
// createRoot mounting and jsx transformation) and do not enlarge the
// whitelisted package set.
// **The build path has fully moved out of rollup; esbuildDirectBundle builds
// these directly**: these shims are multi-entries with zero static consumers,
// so rollup tree-shaking would drop all the entry export bindings (the
// react-family CJS interop assignment statements are no longer treated as
// surviving side effects under vite 6, leaving artifacts as bare CJS carriers
// with no exports — every browser static import lands empty, the root cause
// of white screens; the rxjs/slsdk-ui `export *` module graphs are empty for
// the same reason). The direct-build configuration is in esbuildDirectBundle.
// **This object serves the dev path only** (the vendorDevServer's
// transformRequest compiles the shim sources on the fly; dev has no bundle
// stage and thus no tree-shaking problem).
const vendorEntries = {
  react: resolve(root, 'src/vendor/react.ts'),
  'react-dom': resolve(root, 'src/vendor/react-dom.ts'),
  'react-dom-client': resolve(root, 'src/vendor/react-dom-client.ts'),
  'react-jsx-runtime': resolve(root, 'src/vendor/react-jsx-runtime.ts'),
  'react-jsx-dev-runtime': resolve(root, 'src/vendor/react-jsx-dev-runtime.ts'),
};

// The slsdk-ts ui entry source (@shell/sdk/ui): shared UI facilities
// (component families/hooks/CSS contract facilities). CSS is carried in a TS
// template string (css.ts's uiCssText), so the source module graph is pure
// JS/TS and direct esbuild bundling has no CSS extraction problem.
const SLSDK_UI_ENTRY = resolve(root, '../slsdk-ts/src/ui/index.ts');
// Whitelisted bare imports kept in the ui artifact (aligned with slsdk-ts
// rslib externals: whitelisted shared libraries are never inlined, preventing
// duplicate instances). The current ui module graph actually only imports
// react/react-dom; rxjs is aligned with the configuration preemptively so a
// future hooks-driven rxjs import is not inlined into a second instance.
const SLSDK_UI_EXTERNAL = ['react', 'react-dom', 'rxjs'];

// Dev middleware: the import map's URLs (/vendor/*.js) have no build
// artifacts in dev mode, so vite's transformRequest compiles the shim sources
// (src/vendor/*.ts) to ESM on the fly.
// Imports of whitelisted libraries inside the output are rewritten by vite to
// pre-bundled dependency paths, which the browser then loads normally.
// The import map URLs have the same shape in both dev and build modes — no
// need for two configurations.
// The rxjs dev path goes through the shim source (dev has no bundle stage and
// thus no tree-shaking problem); the build path is produced by
// esbuildDirectBundle (see the vendorEntries comment).
// **The @shell/sdk/ui dev path does not go through transformRequest**: it is
// a real source module graph with relative submodules (./components etc.),
// and after single-file transformation relative imports would resolve against
// the /vendor/ URL as base and 404 — so it uses esbuild in-memory direct
// bundling with the same configuration as the build path (write:false, see
// bundleSlsdkUiForDev); both paths' artifacts are isomorphic and their true
// source is the slsdk-ts source in both cases.
const devVendorEntries: Record<string, string> = {
  ...vendorEntries,
  rxjs: resolve(root, 'src/vendor/rxjs.ts'),
};

// The @shell/sdk/ui dev artifact: esbuild in-memory direct bundling
// (write:false, nothing on disk), same configuration as the ui branch of the
// build path's esbuildDirectBundle — bare react/react-dom/rxjs imports are
// preserved, and the browser resolves them to vendor singletons via the
// import map. After an slsdk-ts source change, the next request picks up the
// new artifact (no cache; direct bundling is ms-scale, negligible cost for a
// low-frequency change scenario).
async function bundleSlsdkUiForDev(): Promise<string> {
  const esbuild = await import('esbuild');
  const result = await esbuild.build({
    entryPoints: { 'vendor/slsdk-ui': SLSDK_UI_ENTRY },
    bundle: true,
    format: 'esm',
    // JSX uses the automatic runtime (the artifact imports
    // "react/jsx-runtime", already a key in the import map); declared
    // explicitly so we don't depend on the esbuild version's default
    jsx: 'automatic',
    external: SLSDK_UI_EXTERNAL,
    outdir: resolve(root, 'dist'),
    write: false,
    logLevel: 'silent',
  });
  const file = result.outputFiles?.find((f) => f.path.endsWith('slsdk-ui.js'));
  if (!file) throw new Error('slsdk-ui dev bundle output missing');
  return file.text;
}

function vendorDevServer(): Plugin {
  return {
    name: 'vendor-dev-server',
    apply: 'serve',
    configureServer(server) {
      server.middlewares.use((req, res, next) => {
        const match = req.url?.match(/^\/vendor\/([\w.-]+)\.js(?:\?.*)?$/);
        if (!match) return next();
        // @shell/sdk/ui: esbuild in-memory direct bundling (transformRequest
        // does not apply to a module graph with relative submodules, see the
        // devVendorEntries comment)
        if (match[1] === 'slsdk-ui') {
          bundleSlsdkUiForDev()
            .then((code) => {
              res.statusCode = 200;
              res.setHeader('Content-Type', 'application/javascript');
              // Components loading vendors via dynamic import (cross-origin)
              // also need CORS to allow it
              res.setHeader('Access-Control-Allow-Origin', '*');
              res.end(code);
            })
            .catch(() => {
              res.statusCode = 500;
              res.end();
            });
          return;
        }
        const entry = devVendorEntries[match[1]];
        if (!entry) return next();
        server
          .transformRequest(entry)
          .then((result) => {
            if (!result) {
              res.statusCode = 404;
              res.end();
              return;
            }
            res.statusCode = 200;
            res.setHeader('Content-Type', 'application/javascript');
            // Components loading vendors via dynamic import (cross-origin)
            // also need CORS to allow it
            res.setHeader('Access-Control-Allow-Origin', '*');
            res.end(result.code);
          })
          .catch(() => {
            res.statusCode = 500;
            res.end();
          });
      });
    },
  };
}

// The build's singleton guarantee (critical): whitelisted libraries stay
// external to shell code — bare imports are preserved in the artifact and
// resolved at runtime to vendor artifacts by index.html's import map.
// Otherwise the shell main bundle would embed its own react copy, forming a
// dual instance with the vendor react that components load via the import
// map (the hooks dispatcher is a module-level object; a dual instance
// inevitably crashes with Invalid hook call — exactly what the
// architecture.md whitelist decision guards against).
// (Dev mode does not go through this logic: the whitelisted imports of both
// shell and components are rewritten by vite to the same pre-bundled
// dependency, so singletons hold naturally.)
const WHITELIST_IMPORTS = [
  'react',
  'react/jsx-runtime',
  'react/jsx-dev-runtime',
  'react-dom',
  'react-dom/client',
  'rxjs',
];

// Direct-build specs for the react-family shims. For the singleton design see
// the virtualReactExternal plugin: each artifact inlines only its own body,
// and react references are forwarded by the plugin as bare imports for the
// import map to resolve (the react-dom-family packages require('react')
// internally; if inlined, a dual react instance would inevitably break the
// hooks dispatcher); the react body itself has no external dependencies and
// is fully inlined. scheduler is not in the whitelist/mapping and is inlined
// independently with each react-dom-family body (each artifact's module graph
// is self-contained; no mutable state is shared between them).
const REACT_VENDOR_SPECS = [
  { out: 'react', pkg: 'react' },
  { out: 'react-dom', pkg: 'react-dom' },
  { out: 'react-dom-client', pkg: 'react-dom/client' },
  { out: 'react-jsx-runtime', pkg: 'react/jsx-runtime' },
  { out: 'react-jsx-dev-runtime', pkg: 'react/jsx-dev-runtime' },
];

// The react-family shim contents are generated dynamically by enumerating at
// runtime (the key set follows the react version automatically; default is
// exported separately from the import of the body). Bundling the package name
// directly or using export * does not work: esbuild's ESM output for CJS has
// only default; `export * from cjs` gets transformed into a __reExport
// runtime copy — operating on an internal object with no static export
// declaration referencing it, equivalent to zero exports. Meanwhile the
// browser components' and the slsdk-ui artifact's
// `import { jsx } from "react/jsx-runtime"` is an ESM static binding that
// requires the artifact to have static named exports — only an explicit
// named re-export list is statically expanded by esbuild into the artifact's
// exports.
const requireFromShell = createRequire(import.meta.url);
function cjsShimSource(pkg: string): string {
  const named = Object.keys(requireFromShell(pkg)).filter(
    (k) => /^[A-Za-z_$][\w$]*$/.test(k) && k !== 'default',
  );
  return [
    `import vendor from '${pkg}';`,
    `export default vendor;`,
    `export { ${named.join(', ')} } from '${pkg}';`,
  ].join('\n');
}

// react singleton plugin: esbuild's external option does not work in this
// scenario — ① external prefix matching would also hit the
// 'react/jsx-runtime' subpath (the package-name prefix of react.js); ② a CJS
// body's internal require(external) keeps the require call under ESM output
// (browsers have no require; it explodes on load). Instead a resolve plugin:
// only the require('react') inside react-dom-family bodies (kind=require-call)
// is forwarded to a virtual ESM forwarding module — that module marks its
// import of "react" external so it emits a bare import (for the import map to
// lock the singleton) and forwards named exports natively; the entry shim's
// import-statement is not intercepted, so the react body inlines normally
// into the react.js artifact.
function virtualReactExternal(): import('esbuild').Plugin {
  return {
    name: 'virtual-react-external',
    setup(build) {
      build.onResolve({ filter: /^react$/ }, (args) => {
        // An import inside the virtual forwarding module: mark external →
        // bare import (import map singleton)
        if (args.namespace === 'react-virtual') {
          return { path: 'react', external: true };
        }
        // A require inside a react-dom-family body: forward to the virtual module
        if (args.kind === 'require-call') {
          return { path: 'react', namespace: 'react-virtual' };
        }
        // The entry shim's import-statement: resolve normally; react body inlines
        return null;
      });
      build.onLoad({ filter: /.*/, namespace: 'react-virtual' }, () => ({
        contents: `import vendor from "react";\nexport default vendor;\nexport * from "react";\n`,
        resolveDir: root,
      }));
    },
  };
}

// Direct-build plugin (build): rollup's tree-shaking of zero-static-consumer
// entries is unfixable (see the vendorEntries comment), so all vendor
// artifacts are built by esbuild as single-file ESM into dist/vendor/ — the
// import map's URLs have the same shape in both modes. esbuild is a built-in
// vite dependency: zero new packages.
function esbuildDirectBundle(): Plugin {
  return {
    name: 'esbuild-direct-bundle',
    apply: 'build',
    async closeBundle() {
      const esbuild = await import('esbuild');
      const base = {
        bundle: true,
        format: 'esm',
        outdir: resolve(root, 'dist'),
        // Browsers have no process: react-family package entries branch on
        // NODE_ENV to pick an implementation, which must be statically
        // eliminated — otherwise the artifact throws ReferenceError on load
        // and both dev/prod implementations end up in the artifact. rxjs and
        // slsdk-ui have no references to it; harmless.
        define: { 'process.env.NODE_ENV': '"production"' },
        logLevel: 'silent',
      };
      await esbuild.build({ ...base, entryPoints: { 'vendor/rxjs': 'rxjs' } });
      await esbuild.build({
        ...base,
        entryPoints: { 'vendor/slsdk-ui': SLSDK_UI_ENTRY },
        jsx: 'automatic',
        external: SLSDK_UI_EXTERNAL,
      });
      for (const spec of REACT_VENDOR_SPECS) {
        await esbuild.build({
          ...base,
          plugins: [virtualReactExternal()],
          stdin: {
            contents: cjsShimSource(spec.pkg),
            sourcefile: `${spec.out}.ts`,
            resolveDir: root,
          },
          entryNames: `vendor/${spec.out}`,
        });
      }
    },
  };
}

export default defineConfig({
  plugins: [react(), vendorDevServer(), esbuildDirectBundle()],
  // tauri dev mode uses a fixed port, avoiding config changes every time
  clearScreen: false,
  server: {
    port: 5173,
    strictPort: true,
  },
  build: {
    rollupOptions: {
      input: {
        main: resolve(root, 'index.html'),
      },
      // Whitelisted libraries stay external to shell code (bare imports are
      // handed to the import map, see the WHITELIST_IMPORTS comment). Vendor
      // artifacts do not go through rollup (all built directly by esbuild,
      // see esbuildDirectBundle), so no shim-exemption logic is needed.
      external(source) {
        return WHITELIST_IMPORTS.includes(source);
      },
      output: {
        chunkFileNames: 'assets/chunk-[hash].js',
        assetFileNames: 'assets/[name]-[hash][extname]',
      },
    },
  },
});
