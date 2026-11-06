// Build entry (the vite rollup input): default-exports the component root.
// The shell's module tier dynamically imports this artifact (fixed name entry.js,
// statically declared by manifest.ui.entry). Zero side effects here; styles ride
// along with the component code.
export { default } from './App';
