import i18next from 'i18next';
import { registerI18n } from '@shell/sdk/ui';

// Namespace constraint (for the component author and any AI assistant working
// on this project): this namespace (`__NS__`) is pre-wired by the scaffolder
// and reserved for this component. The `menuTitle` key inside it is managed by
// the superlive shell — the shell injects the manifest's localized menu titles
// into this namespace at registration time, so do NOT define `menuTitle` here.
// Project keys: every locale configured at creation is pre-wired here; extend
// freely per locale (the shell resolves the requested locale → zh → en). Every
// locale is registered upfront at module load, so a shell language switch
// applies instantly.

// Standalone (vite dev) there is no shared instance yet — init a local default so
// useTranslation works; inside the shell this module resolves to the already
// initialized shared instance and isInitialized skips the local init.
if (!i18next.isInitialized) {
  void i18next.init({ lng: 'en', fallbackLng: 'en', defaultNS: '__NS__', resources: {} });
}

// Register this component's namespace (all locales, upfront, at module load):
// language switches are instant with zero component cooperation. Inside the shell
// the bundles land on the shared i18next instance; standalone they land on the
// local one initialized above.
registerI18n('__NS__', {
__TITLE_ROWS__
});
