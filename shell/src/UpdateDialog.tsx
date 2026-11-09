// App self-update dialog: a frontend-driven flow over the Rust updater
// plugin — check → available (version + notes) → download & install
// (progress) → relaunch straight into the new build.
//
// The "Finished" event only means the download finished — on macOS and
// Linux the bundle swap (install) starts after it, inside the same
// downloadAndInstall call. Relaunching from the event would kill the
// install mid-flight and restart the old build, so the relaunch waits
// for the promise to resolve (install actually completed). If the
// relaunch call fails, the dialog falls back to an "installed" prompt
// for a manual restart.

import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { check, type Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import { Modal } from "./Modal";

type Phase =
  "checking" | "available" | "uptodate" | "downloading" | "done" | "error";

export function UpdateDialog({ onClose }: { onClose: () => void }) {
  const { t } = useTranslation();
  const [phase, setPhase] = useState<Phase>("checking");
  const [update, setUpdate] = useState<Update | null>(null);
  const [received, setReceived] = useState(0);
  const [total, setTotal] = useState(0);
  const [error, setError] = useState("");

  useEffect(() => {
    let alive = true;
    check()
      .then((u) => {
        if (!alive) return;
        if (u) {
          setUpdate(u);
          setPhase("available");
        } else {
          setPhase("uptodate");
        }
      })
      .catch((e) => {
        if (!alive) return;
        setError(String(e));
        setPhase("error");
      });
    return () => {
      alive = false;
    };
  }, []);

  const startDownload = () => {
    if (!update) return;
    setPhase("downloading");
    let got = 0;
    update
      .downloadAndInstall((event) => {
        switch (event.event) {
          case "Started":
            setTotal(event.data.contentLength ?? 0);
            break;
          case "Progress":
            got += event.data.chunkLength;
            setReceived(got);
            break;
          case "Finished":
            // Download done — the install (bundle swap) runs next inside
            // the same call; keep the progress UI until it completes.
            break;
        }
      })
      .then(() => {
        // The promise resolves after the install completed — the new
        // build is in place. Restart straight into it; on relaunch
        // failure fall back to the manual-restart prompt.
        setPhase("done");
        relaunch().catch((e) => {
          setError(String(e));
        });
      })
      .catch((e) => {
        setError(String(e));
        setPhase("error");
      });
  };

  const pct =
    total > 0 ? Math.min(100, Math.round((received / total) * 100)) : null;

  return (
    <Modal
      title={t("settings.check")}
      footer={
        <>
          {phase === "available" && (
            <button type="button" onClick={startDownload}>
              {t("update.install", { version: update?.version ?? "" })}
            </button>
          )}
          {(phase === "available" ||
            phase === "uptodate" ||
            phase === "done" ||
            phase === "error") && (
            <button type="button" onClick={onClose}>
              {t("update.close")}
            </button>
          )}
        </>
      }
    >
      {phase === "checking" && <p>{t("update.checking")}</p>}
      {phase === "available" && (
        <div>
          <p>{t("update.available", { version: update?.version ?? "" })}</p>
          {update?.body && <pre className="update-notes">{update.body}</pre>}
        </div>
      )}
      {phase === "uptodate" && <p>{t("update.uptodate")}</p>}
      {phase === "downloading" && (
        <div>
          <p>
            {t("update.downloading")}{" "}
            {pct !== null
              ? `${pct}%`
              : `${(received / 1024 / 1024).toFixed(1)} MB`}
          </p>
          <div className="update-progress">
            {pct !== null && <div style={{ width: `${pct}%` }} />}
          </div>
        </div>
      )}
      {phase === "done" && <p>{t("update.installed")}</p>}
      {phase === "error" && <p>{t("update.failed", { error })}</p>}
    </Modal>
  );
}
