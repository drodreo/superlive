// Activity bar — the workspace's top strip, owned by the shell framework
// (not by components). Left/right symmetric zones: current module title /
// live date-time / theme toggle.

import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';

export type Theme = 'light' | 'dark';

function formatClock(d: Date): string {
  const p = (n: number) => String(n).padStart(2, '0');
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}`;
}

export function ActivityBar({
  title,
  theme,
  onToggleTheme,
}: {
  title: string;
  theme: Theme;
  onToggleTheme: () => void;
}) {
  const { t } = useTranslation();
  const [now, setNow] = useState(() => new Date());

  // Live clock: ticks every second, displays minute precision
  useEffect(() => {
    const timer = setInterval(() => setNow(new Date()), 1000);
    return () => clearInterval(timer);
  }, []);

  return (
    <header className="activity-bar">
      <div className="activity-left">{title}</div>
      <div className="activity-center">{formatClock(now)}</div>
      <div className="activity-right">
        <button type="button" onClick={onToggleTheme}>
          {theme === 'light' ? t('theme.toDark') : t('theme.toLight')}
        </button>
      </div>
    </header>
  );
}
