// Vendor shim: the react whitelist entry (import map "react" → /vendor/react.js).
// The default export points at the namespace (component code
// `import React from 'react'` gets the full object),
// and the named re-export covers the `import { useState } from 'react'` form.
// (@types/react is an export = declaration, so export * is invalid at the
// type level; at runtime react 19's ESM named exports are complete — the shim
// only serves as the import map's singleton anchor, suppressing type noise)
import * as React from 'react';

export default React;
// @ts-expect-error see above: the type declaration's form differs from the runtime ESM's
export * from 'react';
