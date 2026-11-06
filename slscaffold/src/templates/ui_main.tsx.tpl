import React from 'react';
import ReactDOM from 'react-dom/client';
import App from './App';

// Standalone debug entry (vite dev); NOT part of the build output (the rollup input
// is entry.tsx). Fake the shell injection so the component runs outside the shell:
// the empty origin makes API paths relative, and vite's dev proxy forwards them to
// the local backend started with a fixed SL_ENDPOINT port.
if (!window.__SL_SHELL__) {
  window.__SL_SHELL__ = { origin: '', token: '' };
}

ReactDOM.createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    <App route="/" />
  </React.StrictMode>,
);
