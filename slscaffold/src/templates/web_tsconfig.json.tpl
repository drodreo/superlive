{
  "compilerOptions": {
    "target": "ES2020",
    "useDefineForClassFields": true,
    "lib": ["ES2020", "DOM", "DOM.Iterable"],
    "module": "ESNext",
    "skipLibCheck": true,

    "moduleResolution": "bundler",
    "resolveJsonModule": true,
    "isolatedModules": true,
    "noEmit": true,
    "jsx": "react-jsx",

    // '@shell/sdk' / '@shell/sdk/ui' are import-map keys (source imports use these
    // facade names). TS paths must point at concrete files: a bare package name does
    // not go through node_modules resolution, and a directory only probes index
    // files without honoring package.json exports. Pointing at the SDK source
    // entries makes types = source, independent of the dist layout. '/ui' is an
    // exact-match entry — without a wildcard it does not hit the '@shell/sdk' rule,
    // so it must be listed separately.
    "baseUrl": ".",
    "paths": {
      "@shell/sdk": ["./node_modules/slsdk-ts/src/index.ts"],
      "@shell/sdk/ui": ["./node_modules/slsdk-ts/src/ui/index.ts"]
    },

    "strict": true,
    "noUnusedLocals": true,
    "noUnusedParameters": true,
    "noFallthroughCasesInSwitch": true
  },
  "include": ["src"]
}
