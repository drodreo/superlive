// Shared test setup: a minimal i18n instance.
//
// SurfaceHost renders its phases through react-i18next's useTranslation; the
// real shell i18n (src/i18n.ts) pulls locale resources and localStorage —
// the tests only need the host.* keys they assert on, bound to the same
// global i18next instance react-i18next falls back to. Top-level await keeps
// init blocking until ready, so translations exist at first render.
import i18n from "i18next";
import { initReactI18next } from "react-i18next";

await i18n.use(initReactI18next).init({
  lng: "en",
  fallbackLng: "en",
  resources: {
    en: {
      translation: {
        "host.loading": "Loading…",
        "host.renderFailed": "Component failed to load or render: {{error}}",
        "host.surfaceLoadFailed":
          "Component description failed to load: {{error}}",
        "host.retry": "Retry",
      },
    },
  },
});
