#@if HAS_SERVER
import { useEffect, useState } from 'react';
#@endif
import { useTranslation } from 'react-i18next';
#@if HAS_SERVER
import { createClient } from '@shell/sdk';
#@endif
import { useShellState } from '@shell/sdk/ui';
import './i18n';

#@if HAS_SERVER
/**
 * This component's namespace prefix on the shell gateway: the shell strips it before
 * forwarding, so the backend always receives its own full paths. The packaging script
 * greps this constant to keep the manifest and the UI in sync.
 */
const NAMESPACE_PREFIX = '/api/__NS__';

interface HelloResponse {
  message: string;
}
#@endif

/**
 * Component root. The shell's module tier mounts the default export via React.lazy
 * and passes the current route path verbatim (ComponentModuleProps).
 */
export default function App({ route }: { route: string }) {
  // Shell-injection readiness: 'waiting' while the async injection lands (render
  // null to avoid flashing a fallback), 'missing' after the timeout.
  const shell = useShellState();
  const { t } = useTranslation('__NS__');
#@if HAS_SERVER
  const [message, setMessage] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (shell !== 'ready') return;
    const env = window.__SL_SHELL__;
    if (!env) return;
    const api = createClient({ baseUrl: `${env.origin}${NAMESPACE_PREFIX}`, token: env.token });
    let cancelled = false;
    api
      .request('GET', '/api/v1/hello')
      .then((res) => res.json<HelloResponse>())
      .then((data) => {
        if (!cancelled) setMessage(data.message);
      })
      .catch((err) => {
        if (!cancelled) setError(String(err));
      });
    return () => {
      cancelled = true;
    };
  }, [shell]);
#@endif

  if (shell === 'missing') {
    return (
      <div style={{ padding: 24 }}>
        <h1>{t('title')}</h1>
        <p>This component must run inside the superlive shell (missing the injected runtime environment).</p>
      </div>
    );
  }
  if (shell === 'waiting') return null;

  return (
    <div style={{ padding: 24 }}>
      <h1>{t('title')}</h1>
      <p>route: {route}</p>
#@if HAS_SERVER
      {error ? (
        <p style={{ color: '#c0392b' }}>error: {error}</p>
      ) : message === null ? (
        <p>loading…</p>
      ) : (
        <p>server says: {message}</p>
      )}
#@endif
#@if !HAS_SERVER
      <p>This component has no backend process — replace this sample UI with your own.</p>
#@endif
    </div>
  );
}
