// Welcome page — the neutral landing when no menu entry is selected (first
// entry with an empty hash, or an unrecognized path). Informational v1:
// what the app is and where things live in the menu.

import { useTranslation } from 'react-i18next';

export function WelcomePage() {
  const { t } = useTranslation();

  return (
    <section className="welcome">
      <div>
        <h2>{t('welcome.title')}</h2>
        <p>{t('welcome.desc')}</p>
      </div>
    </section>
  );
}
