// Settings page — the target of the sidebar's bottom menu entry. Hosts
// app-level cards: self-update (current version + check via the updater
// dialog) and interface language; future settings slot in as cards.

import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { getVersion } from "@tauri-apps/api/app";
import { setLocale } from "./i18n";
import { UpdateDialog } from "./UpdateDialog";

export function SettingsPage() {
  const { t, i18n } = useTranslation();
  const [version, setVersion] = useState("");
  const [showUpdate, setShowUpdate] = useState(false);
  // Language options render native names (中文 / English) by convention
  const locale = i18n.language === "zh" ? "zh" : "en";

  // Current app version (the Cargo.toml package version — the config omits
  // `version`), shown in the update card
  useEffect(() => {
    let alive = true;
    getVersion()
      .then((v) => {
        if (alive) setVersion(v);
      })
      .catch(() => {
        /* version display is best-effort; the update flow shows its own errors */
      });
    return () => {
      alive = false;
    };
  }, []);

  return (
    <section className="settings">
      <h2>{t("menu.settings")}</h2>

      <div className="settings-card">
        <div className="settings-card-head">
          <h3>{t("settings.update")}</h3>
          <button type="button" onClick={() => setShowUpdate(true)}>
            {t("settings.check")}
          </button>
        </div>
        <p className="settings-hint">
          {t("settings.updateHint", { version: version || "…" })}
        </p>
      </div>

      <div className="settings-card">
        <div className="settings-card-head">
          <h3>{t("settings.language")}</h3>
          <div className="locale-switch">
            <button
              type="button"
              className={locale === "zh" ? "on" : ""}
              onClick={() => setLocale("zh")}
            >
              中文
            </button>
            <button
              type="button"
              className={locale === "en" ? "on" : ""}
              onClick={() => setLocale("en")}
            >
              English
            </button>
          </div>
        </div>
        <p className="settings-hint">{t("settings.languageHint")}</p>
      </div>

      {showUpdate && <UpdateDialog onClose={() => setShowUpdate(false)} />}
    </section>
  );
}
