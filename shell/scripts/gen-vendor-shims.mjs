#!/usr/bin/env node
/**
 * Regenerates the react-family dev vendor shims (src/vendor/react*.ts).
 *
 * Why property-extraction form: vite's pre-bundled CJS-interop artifacts
 * expose ONLY a default export (the CJS namespace object) — `export *` and
 * static re-exports (`export { x } from 'pkg'`) forward zero named bindings,
 * so cross-origin component bundles statically importing named bindings
 * (`{ jsx }`, `{ Fragment }`, `{ useState }`…) fail module linking at
 * `import map → /vendor/*.js`. Default import + `export const x = vendor.x`
 * declares the named exports statically on the shim itself and reads the
 * properties at runtime — the same lesson as cjsShimSource in
 * vite.config.ts (build path, unaffected by this script).
 *
 * Run from shell/ after a react/react-dom version bump:
 *   node scripts/gen-vendor-shims.mjs
 */
import { createRequire } from 'node:module';
import { writeFile } from 'node:fs/promises';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = dirname(dirname(fileURLToPath(import.meta.url)));
const require = createRequire(import.meta.url);

const SPECS = [
  { file: 'react.ts', pkg: 'react', note: 'the react whitelist entry' },
  { file: 'react-dom.ts', pkg: 'react-dom', note: 'the react-dom whitelist entry' },
  {
    file: 'react-dom-client.ts',
    pkg: 'react-dom/client',
    note: 'the react-dom/client subpath (createRoot mounting)',
  },
  {
    file: 'react-jsx-runtime.ts',
    pkg: 'react/jsx-runtime',
    note: 'the react/jsx-runtime subpath (production jsx transform runtime)',
  },
  {
    file: 'react-jsx-dev-runtime.ts',
    pkg: 'react/jsx-dev-runtime',
    note: 'the react/jsx-dev-runtime subpath (dev jsx transform runtime)',
  },
];

for (const spec of SPECS) {
  const vendor = require(spec.pkg);
  const names = Object.keys(vendor).filter(
    (k) => k !== 'default' && /^[A-Za-z_$][\w$]*$/.test(k),
  );
  const body = names
    .map((n, i) => `  ${n} = vendor.${n}${i < names.length - 1 ? ',' : ';'}`)
    .join('\n');
  const content = `// Vendor shim (GENERATED — do not hand-edit; regenerate with
// \`node scripts/gen-vendor-shims.mjs\` after a react/react-dom bump):
// ${spec.note}.
// Property-extraction form: vite's pre-bundled CJS-interop artifacts expose
// only a default export, so export * / static re-exports forward zero named
// bindings — cross-origin component bundles statically importing named
// bindings would fail module linking. The build path (esbuildDirectBundle,
// cjsShimSource) is unaffected. The accessor is cast: a few runtime-only
// keys have no @types counterparts.
import vendorImport from '${spec.pkg}';

const vendor = vendorImport as unknown as Record<string, unknown>;

export default vendorImport;

export const
${body}
`;
  await writeFile(resolve(root, 'src/vendor', spec.file), content);
  console.log(`${spec.file}: ${names.length} named exports`);
}
