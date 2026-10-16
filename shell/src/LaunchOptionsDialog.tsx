// Launch options dialog (architecture.md "Launch options slot"):
// Timing = after configuration completes and validation passes — the shell's
// "launch options" prompt step; this step is an extensible slot — the current
// first item = launch policy (auto/manual), and future launch options append
// their option sections inside this component (independent state), submitted
// in order via confirm; the parent then runs start_component,
// leaving the lifecycle main flow untouched (install → config gate → launch
// options → start).

import { useState } from "react";
import { firstValueFrom } from "rxjs";
import { useTranslation } from "react-i18next";
import { setLaunchPolicy, type LaunchPolicy } from "./api";
import { Modal } from "./Modal";
import type { ShellKey } from "./i18n";

export function LaunchOptionsDialog({
  id,
  onDone,
  onCancel,
}: {
  id: string;
  /** All slot options submitted (the policy persisted); the parent performs the start. */
  onDone: () => void;
  /** The user declines: no options saved; the component stays in its current state. */
  onCancel: () => void;
}) {
  const { t } = useTranslation();
  // --- Slot 1: launch policy (component-level, persisted in the shell-side registry, not in the component manifest) ---
  const [policy, setPolicy] = useState<LaunchPolicy>("auto");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<{
    key: ShellKey;
    params?: Record<string, string | number>;
  } | null>(null);

  // Confirm = run each slot option's command call in order (currently only
  // the launch policy)
  const confirm = async () => {
    setBusy(true);
    setError(null);
    try {
      await firstValueFrom(setLaunchPolicy(id, policy));
      onDone();
    } catch (e) {
      setError({ key: "launch.policyFailed", params: { error: String(e) } });
    } finally {
      setBusy(false);
    }
  };

  return (
    <Modal
      title={t("launch.title")}
      footer={
        <>
          <button type="button" disabled={busy} onClick={onCancel}>
            {t("launch.later")}
          </button>
          <button
            type="button"
            className="primary"
            disabled={busy}
            onClick={() => void confirm()}
          >
            {t("launch.saveAndStart")}
          </button>
        </>
      }
    >
      {error && (
        <div className="message error">{t(error.key, error.params)}</div>
      )}
      <div className="launch-option">
        <div className="launch-option-title">{t("launch.policy")}</div>
        <label>
          <input
            type="radio"
            name={`launch-policy-${id}`}
            checked={policy === "auto"}
            onChange={() => setPolicy("auto")}
          />
          {t("launch.auto")}
        </label>
        <label>
          <input
            type="radio"
            name={`launch-policy-${id}`}
            checked={policy === "manual"}
            onChange={() => setPolicy("manual")}
          />
          {t("launch.manual")}
        </label>
      </div>
      {/* Future launch-options slots: append option sections here (independent state, submitted in order via confirm) */}
    </Modal>
  );
}
