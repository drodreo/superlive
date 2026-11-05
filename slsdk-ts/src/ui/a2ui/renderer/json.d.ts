/**
 * Ambient type for JSON imports used by the renderer. The catalog document is
 * data imported at build time (rslib / vite bundle it; tests resolve it via
 * vite); the tsconfig intentionally stays without resolveJsonModule, so the
 * module is typed as `unknown` here and cast at the single consumption site.
 */
declare module "*.json" {
  const value: unknown;
  export default value;
}
