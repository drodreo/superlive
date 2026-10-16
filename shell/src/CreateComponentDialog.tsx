// Create-component dialog (architecture.md "built-in creation flow", phase
// 3b): a form over the create_component command — id / localized display
// titles (locale-selector + title-input rows; the single-language display
// name is derived at assembly) / namespace / capability checkboxes (entry +
// launch, the combination is the shape) / project location / advanced options
// (dev port, SDK mode).
//
// Error branching follows the command contract (commands.rs CreateError):
// the rejection string is the JSON {"kind", "message"} — invalid_argument
// flags the form field the message points at (the keyword mapping mirrors the
// Rust-side message texts), conflict and internal surface as top-of-form
// messages with the original text passed through (the established shell
// convention: error strings render verbatim).
//
// Local error visibility follows the touched pattern: a local error only
// shows after its field was left (blur) or a submit was attempted — the
// dialog opens with zero red even though every required value is still
// empty, and Create validates on click instead of being disabled.
//
// Project location is a composite path control: the input stays freely
// typable and the embedded trailing button opens the OS directory picker
// (tauri-plugin-dialog) and fills the path.

import { useState } from "react";
import { firstValueFrom } from "rxjs";
import { useTranslation } from "react-i18next";
import { open } from "@tauri-apps/plugin-dialog";
import {
  createComponent,
  importComponent,
  type BackendTemplate,
  type ComponentShape,
  type CreateComponentResult,
  type SdkMode,
  type UpgradeItem,
  type UpgradeKind,
} from "./api";
import { Modal } from "./Modal";
import type { ShellKey } from "./i18n";

/** Shared charset of id / namespace (the shell Rust side validates the same rule). */
const ID_PATTERN = /^[A-Za-z0-9-]+$/;

/** Fixed common-locale set of the title rows' selector (extensible by editing this list). */
const TITLE_LOCALES = ["zh", "en", "ja", "ko", "fr", "de", "es", "ru"] as const;

/** One localized-title row of the dialog form (touched gates the row's error display). */
interface TitleRow {
  locale: string;
  value: string;
  touched: boolean;
}

/** Form fields an invalid_argument can be pinned to. */
type FieldKey = "id" | "namespace" | "devPort" | "sdkPath";

/** Fields whose local errors follow the touched/submitAttempted visibility gate. */
type TouchableField = "id" | "namespace" | "targetDir" | "devPort" | "sdkPath";

/** The documented rejection JSON (commands.rs CreateError's serialization). */
interface CommandError {
  kind: "invalid_argument" | "conflict" | "internal";
  message: string;
}

/** Parse the rejection defensively; anything but the documented JSON → null (generic failure). */
function parseCommandError(raw: unknown): CommandError | null {
  try {
    const parsed = JSON.parse(String(raw)) as Partial<CommandError>;
    if (
      (parsed.kind === "invalid_argument" ||
        parsed.kind === "conflict" ||
        parsed.kind === "internal") &&
      typeof parsed.message === "string"
    ) {
      return { kind: parsed.kind, message: parsed.message };
    }
  } catch {
    // fall through
  }
  return null;
}

/** First selector locale not used by any row yet (null = all fixed locales are taken). */
function nextFreeLocale(rows: TitleRow[]): string | null {
  const used = new Set(rows.map((row) => row.locale));
  return TITLE_LOCALES.find((locale) => !used.has(locale)) ?? null;
}

/** i18n key per upgrade-item kind (the confirmation list's human label; the detail renders verbatim). */
const UPGRADE_KIND_KEYS: Record<UpgradeKind, ShellKey> = {
  derive_titles: "dlg.up.deriveTitles",
  refresh_version: "dlg.up.refreshVersion",
  fill_min_shell_version: "dlg.up.fillMinShellVersion",
  add_build_script: "dlg.up.addBuildScript",
  add_readme: "dlg.up.addReadme",
};

/**
 * invalid_argument → the form field the message points at. The keywords
 * mirror the Rust-side message texts (slscaffold's InvalidId / InvalidNamespace /
 * InvalidDevPort displays and parse_sdk's "requires sdk_path"), matched
 * most-specific first; unknown shapes/sdk_modes cannot originate from this
 * dialog, so anything else stays top-of-form.
 */
function fieldOfMessage(message: string): FieldKey | null {
  if (/sdk_path/.test(message)) return "sdkPath";
  if (/dev[ _-]?port/i.test(message)) return "devPort";
  if (/namespace/i.test(message)) return "namespace";
  if (/\bid\b/i.test(message)) return "id";
  return null;
}

export function CreateComponentDialog({
  onCreated,
  onCancel,
}: {
  /** Creation succeeded: the command result (registered view + guide content); the parent closes this dialog and shows the guide. */
  onCreated: (result: CreateComponentResult) => void;
  /** The user declines: nothing was created. */
  onCancel: () => void;
}) {
  const { t } = useTranslation();

  // --- First-step choice (architecture.md "Creation flow"): import an
  // existing project or create fresh. `fresh` carries the original form
  // below unchanged; `import` is the directory-picker flow. ---
  const [step, setStep] = useState<"choose" | "fresh" | "import">("choose");
  // --- Import flow state ---
  const [importDir, setImportDir] = useState("");
  const [importBusy, setImportBusy] = useState(false);
  // Non-null = phase 1 came back `upgrade_required`: the confirmation list
  // is on screen and the footer swaps to apply/cancel
  const [upgradeItems, setUpgradeItems] = useState<UpgradeItem[] | null>(null);
  // Import-side top-of-body message (rejections render the command's reason
  // verbatim; the user may re-pick a directory or close)
  const [importError, setImportError] = useState<{
    key: ShellKey;
    params?: Record<string, string | number>;
  } | null>(null);

  // --- Form fields ---
  const [id, setId] = useState("");
  // Localized title rows: zh/en seeded by default (architecture.md
  // "Localized titles"); every row must carry a non-empty title and a unique
  // locale — at least one row stays
  const [titleRows, setTitleRows] = useState<TitleRow[]>([
    { locale: "zh", value: "", touched: false },
    { locale: "en", value: "", touched: false },
  ]);
  const [namespace, setNamespace] = useState("");
  // Capability checkboxes: both = frontend-backend / entry only = frontend /
  // launch only = backend; neither disables submission
  const [entry, setEntry] = useState(true);
  const [launch, setLaunch] = useState(false);
  // Backend runtime (server shapes only): the radio group shows when the
  // backend checkbox is on; Cargo stays the default
  const [template, setTemplate] = useState<BackendTemplate>("cargo");
  const [targetDir, setTargetDir] = useState("");
  // --- Advanced (collapsed by default) ---
  const [devPort, setDevPort] = useState("8700");
  const [sdkMode, setSdkMode] = useState<SdkMode>("registry");
  const [sdkPath, setSdkPath] = useState("");

  const [busy, setBusy] = useState(false);
  // Top-of-form message (conflict / internal / unattributable invalid_argument)
  const [formError, setFormError] = useState<{
    key: ShellKey;
    params?: Record<string, string | number>;
  } | null>(null);
  // The field an invalid_argument points at (red border)
  const [fieldError, setFieldError] = useState<FieldKey | null>(null);
  // Touched-gate state: a local error only shows once its field was left
  // (blur) or a submit was attempted — an untouched empty form renders zero
  // red, and the failed submit reveals every error at once
  const [submitAttempted, setSubmitAttempted] = useState(false);
  const [touchedFields, setTouchedFields] = useState<
    Record<TouchableField, boolean>
  >({
    id: false,
    namespace: false,
    targetDir: false,
    devPort: false,
    sdkPath: false,
  });

  // --- Local validation (the Rust side stays the authority; local checks gate
  // the submit on click and drive the touched-gated error display below) ---
  const idEmpty = id.trim().length === 0;
  const idInvalid = id.length > 0 && !ID_PATTERN.test(id);
  const nsInvalid = namespace.length > 0 && !ID_PATTERN.test(namespace);
  const targetDirEmpty = targetDir.trim().length === 0;
  const portInvalid =
    !/^\d+$/.test(devPort) || Number(devPort) < 1 || Number(devPort) > 65535;
  const sdkPathEmpty = sdkMode === "local" && sdkPath.trim().length === 0;
  const shape: ComponentShape | null =
    entry && launch
      ? "frontend-backend"
      : entry
        ? "frontend"
        : launch
          ? "backend"
          : null;

  // Title rows: per-row required fields + locale uniqueness. Row flags key by
  // index (rows are never reordered; removal rebuilds the array)
  const rowValueInvalid = titleRows.map((row) => row.value.trim().length === 0);
  const seenLocales = new Set<string>();
  const rowLocaleDuplicate = titleRows.map((row) => {
    const duplicate = seenLocales.has(row.locale);
    seenLocales.add(row.locale);
    return duplicate;
  });
  const titlesInvalid =
    rowValueInvalid.some(Boolean) || rowLocaleDuplicate.some(Boolean);

  // Visibility gates: a local error shows only after its field was left (or
  // any submit was attempted) — an untouched empty form renders zero red
  // (this also aligns id/targetDir's empty-field errors with the title rows':
  // empty + untouched = no error, empty + touched/submitted = error)
  const showFieldError = (field: TouchableField) =>
    touchedFields[field] || submitAttempted;
  const idErrorVisible = (idEmpty || idInvalid) && showFieldError("id");
  const nsErrorVisible = nsInvalid && showFieldError("namespace");
  const targetDirErrorVisible = targetDirEmpty && showFieldError("targetDir");
  const portErrorVisible = portInvalid && showFieldError("devPort");
  const sdkPathErrorVisible = sdkPathEmpty && showFieldError("sdkPath");
  const rowValueErrorVisible = titleRows.map(
    (row, index) => rowValueInvalid[index] && (row.touched || submitAttempted),
  );
  const rowDuplicateErrorVisible = titleRows.map(
    (row, index) =>
      rowLocaleDuplicate[index] && (row.touched || submitAttempted),
  );

  const markTouched = (field: TouchableField) =>
    setTouchedFields((prev) =>
      prev[field] ? prev : { ...prev, [field]: true },
    );
  const markRowTouched = (index: number) =>
    setTitleRows((rows) =>
      rows.map((row, i) =>
        i === index && !row.touched ? { ...row, touched: true } : row,
      ),
    );

  const updateTitleRow = (index: number, next: TitleRow) => {
    setTitleRows((rows) => rows.map((row, i) => (i === index ? next : row)));
  };
  const removeTitleRow = (index: number) => {
    setTitleRows((rows) =>
      rows.length <= 1 ? rows : rows.filter((_, i) => i !== index),
    );
  };
  const addTitleRow = () => {
    setTitleRows((rows) => {
      const locale = nextFreeLocale(rows);
      return locale === null
        ? rows
        : [...rows, { locale, value: "", touched: false }];
    });
  };

  const shapeKey: ShellKey | null =
    shape === "frontend-backend"
      ? "dlg.shapeBoth"
      : shape === "frontend"
        ? "dlg.shapeFrontend"
        : shape === "backend"
          ? "dlg.shapeBackend"
          : "dlg.shapeNone";

  const submit = async () => {
    if (busy) return;
    // Full local validation on click: a failed attempt opens every touched
    // gate at once (all errors visible) and nothing is sent
    const localInvalid =
      idEmpty ||
      idInvalid ||
      nsInvalid ||
      titlesInvalid ||
      shape === null ||
      targetDirEmpty ||
      portInvalid ||
      sdkPathEmpty;
    if (localInvalid) {
      setSubmitAttempted(true);
      setFormError(null);
      setFieldError(null);
      return;
    }
    setBusy(true);
    setFormError(null);
    setFieldError(null);
    try {
      const result = await firstValueFrom(
        createComponent({
          id: id.trim(),
          display_titles: titleRows.map((row) => ({
            locale: row.locale,
            value: row.value.trim(),
          })),
          namespace: namespace.trim() || id.trim(),
          shape,
          // Backend runtime only matters for server shapes: omitted when the
          // backend checkbox is off (the shell defaults to cargo anyway)
          template: launch ? template : undefined,
          target_dir: targetDir.trim(),
          dev_port: Number(devPort),
          sdk_mode: sdkMode,
          sdk_path: sdkMode === "local" ? sdkPath.trim() : null,
        }),
      );
      onCreated(result);
    } catch (e) {
      const err = parseCommandError(e);
      if (!err) {
        setFormError({
          key: "dlg.createFailed",
          params: { message: String(e) },
        });
      } else if (err.kind === "invalid_argument") {
        setFieldError(fieldOfMessage(err.message));
        setFormError({ key: "dlg.invalid", params: { message: err.message } });
      } else if (err.kind === "conflict") {
        setFormError({ key: "dlg.conflict", params: { message: err.message } });
      } else {
        setFormError({
          key: "dlg.createFailed",
          params: { message: err.message },
        });
      }
    } finally {
      setBusy(false);
    }
  };

  // --- Import flow: the OS picker fills the read-only path, phase 1 runs
  // with apply=false, and the confirmation list re-invokes with true. A
  // rejection clears any stale list (the user re-picks or closes). ---
  const pickImportDir = async () => {
    const picked = await open({ directory: true });
    if (typeof picked === "string") {
      // A new directory invalidates the previous inspection
      setUpgradeItems(null);
      setImportError(null);
      setImportDir(picked);
    }
  };

  const runImport = async (apply: boolean) => {
    if (importBusy || importDir.trim().length === 0) return;
    setImportBusy(true);
    setImportError(null);
    try {
      const outcome = await firstValueFrom(importComponent(importDir, apply));
      if (outcome.status === "registered") {
        // Same payload shape as the creation flow — the parent's guide modal
        // consumes it unchanged
        onCreated({
          component: outcome.component,
          project_dir: outcome.project_dir,
          next_steps: outcome.next_steps,
        });
      } else {
        setUpgradeItems(outcome.items);
      }
    } catch (e) {
      setUpgradeItems(null);
      const err = parseCommandError(e);
      if (!err) {
        setImportError({
          key: "dlg.import.failed",
          params: { message: String(e) },
        });
      } else if (err.kind === "internal") {
        setImportError({
          key: "dlg.import.failed",
          params: { message: err.message },
        });
      } else {
        // invalid_argument / conflict: the reason list renders verbatim
        setImportError({
          key: "dlg.import.rejected",
          params: { message: err.message },
        });
      }
    } finally {
      setImportBusy(false);
    }
  };

  const backToChoice = () => {
    setStep("choose");
    setUpgradeItems(null);
    setImportError(null);
    setImportDir("");
  };

  return (
    <Modal
      title={step === "import" ? t("dlg.import.stepTitle") : t("dlg.title")}
      className={step === "fresh" ? "modal-form" : undefined}
      footer={
        step === "choose" ? (
          <button type="button" disabled={busy} onClick={onCancel}>
            {t("dlg.cancel")}
          </button>
        ) : step === "import" ? (
          <>
            <button type="button" disabled={importBusy} onClick={backToChoice}>
              {t("dlg.back")}
            </button>
            {upgradeItems && (
              <button
                type="button"
                className="primary"
                disabled={importBusy}
                onClick={() => void runImport(true)}
              >
                {importBusy ? t("dlg.import.applying") : t("dlg.import.apply")}
              </button>
            )}
          </>
        ) : (
          <>
            <button
              type="button"
              disabled={busy}
              onClick={() => setStep("choose")}
            >
              {t("dlg.back")}
            </button>
            <button
              type="button"
              className="primary"
              disabled={busy}
              onClick={() => void submit()}
            >
              {busy ? t("dlg.creating") : t("dlg.create")}
            </button>
          </>
        )
      }
    >
      {step === "choose" && (
        <div className="dlg-choice-cards">
          <button
            type="button"
            className="dlg-choice-card"
            onClick={() => setStep("import")}
          >
            <span className="dlg-choice-title">
              {t("dlg.card.importTitle")}
            </span>
            <span className="dlg-choice-desc">{t("dlg.card.importDesc")}</span>
          </button>
          <button
            type="button"
            className="dlg-choice-card"
            onClick={() => setStep("fresh")}
          >
            <span className="dlg-choice-title">{t("dlg.card.freshTitle")}</span>
            <span className="dlg-choice-desc">{t("dlg.card.freshDesc")}</span>
          </button>
        </div>
      )}

      {step === "import" && (
        <div className="dlg-fields">
          {importError && (
            <div className="message error">
              {t(importError.key, importError.params)}
            </div>
          )}
          {!upgradeItems ? (
            <>
              <div className="config-field">
                <label htmlFor="dlg-import-dir">
                  {t("dlg.import.dirLabel")}
                </label>
                {/* Composite path control, same form language as the fresh
                    form's location field: a read-only input (import
                    directories come from the OS picker only) with the
                    embedded browse button */}
                <div className="dlg-path-combo">
                  <input
                    id="dlg-import-dir"
                    type="text"
                    value={importDir}
                    readOnly
                    placeholder={t("dlg.import.dirPlaceholder")}
                  />
                  <button
                    type="button"
                    className="dlg-path-browse"
                    title={t("dlg.browse")}
                    aria-label={t("dlg.browse")}
                    onMouseDown={(e) => e.preventDefault()}
                    onClick={() => void pickImportDir()}
                  >
                    …
                  </button>
                </div>
                <div className="dlg-hint">{t("dlg.import.dirHint")}</div>
              </div>
              <div className="dlg-import-actions">
                <button
                  type="button"
                  className="primary"
                  disabled={importDir.trim().length === 0 || importBusy}
                  onClick={() => void runImport(false)}
                >
                  {importBusy
                    ? t("dlg.import.checking")
                    : t("dlg.import.inspect")}
                </button>
              </div>
            </>
          ) : (
            <div className="dlg-import-upgrades">
              <div className="dlg-import-title">
                {t("dlg.import.upgradeTitle")}
              </div>
              <div className="dlg-hint">{t("dlg.import.upgradeHint")}</div>
              <ul className="dlg-import-list">
                {upgradeItems.map((item) => (
                  <li key={`${item.kind}:${item.detail}`}>
                    <span className="dlg-import-kind">
                      {t(UPGRADE_KIND_KEYS[item.kind])}
                    </span>
                    <span className="dlg-import-detail">{item.detail}</span>
                  </li>
                ))}
              </ul>
            </div>
          )}
        </div>
      )}

      {step === "fresh" && (
        <div className="dlg-fields">
          {formError && (
            <div className="message error">
              {t(formError.key, formError.params)}
            </div>
          )}

          <div
            className={`config-field${idErrorVisible || fieldError === "id" ? " has-error" : ""}`}
          >
            <label htmlFor="dlg-id">
              <span className="req">*</span>
              {t("dlg.idLabel")}
            </label>
            <input
              id="dlg-id"
              type="text"
              value={id}
              placeholder="my-component"
              onChange={(e) => setId(e.target.value)}
              onBlur={() => markTouched("id")}
            />
            {idErrorVisible && idEmpty && (
              <div className="dlg-hint error">{t("dlg.idRequired")}</div>
            )}
            <div
              className={`dlg-hint${idErrorVisible && !idEmpty ? " error" : ""}`}
            >
              {t("dlg.idHint")}
            </div>
          </div>

          <div className="config-field">
            <label htmlFor="dlg-titles">
              <span className="req">*</span>
              {t("dlg.titlesLabel")}
            </label>
            <div className="dlg-titles-rows">
              {titleRows.map((row, index) => (
                <div className="dlg-title-row" key={index}>
                  {/* Composite control: the locale picker is embedded as the
                    input's prefix inside one border — a single visual unit
                    per row instead of two side-by-side controls. Row-level
                    errors paint the composite border */}
                  <div className="dlg-title-combo">
                    <select
                      id={index === 0 ? "dlg-titles" : undefined}
                      aria-label={t("dlg.titlesLocale")}
                      className="dlg-title-locale"
                      value={row.locale}
                      onChange={(e) => {
                        // Picking a locale is an explicit interaction: flagging
                        // the row touched shows a clash immediately
                        markRowTouched(index);
                        updateTitleRow(index, {
                          ...row,
                          locale: e.target.value,
                        });
                      }}
                    >
                      {/* Options stay enabled even when picked by another row — the
                      duplicate-locale hint flags the clash instead of hiding it */}
                      {TITLE_LOCALES.map((locale) => (
                        <option key={locale} value={locale}>
                          {locale}
                        </option>
                      ))}
                    </select>
                    <input
                      type="text"
                      aria-label={t("dlg.titlesValue")}
                      value={row.value}
                      placeholder={t("dlg.titlesValue")}
                      onChange={(e) =>
                        updateTitleRow(index, { ...row, value: e.target.value })
                      }
                      onBlur={() => markRowTouched(index)}
                    />
                  </div>
                  <button
                    type="button"
                    className="dlg-title-remove"
                    disabled={titleRows.length <= 1}
                    title={t("dlg.titlesRemove")}
                    aria-label={t("dlg.titlesRemove")}
                    onClick={() => removeTitleRow(index)}
                  >
                    ✕
                  </button>
                  {/* Error line under the input: rendered only while an error
                    is actually visible (the visibility itself is
                    touched/submit-gated). No reserved placeholder — the
                    block stays compact, layout shifts only carry the
                    cause-and-effect of an error appearing or clearing */}
                  {(rowValueErrorVisible[index] ||
                    rowDuplicateErrorVisible[index]) && (
                    <div className="dlg-title-row-error" aria-live="polite">
                      {rowValueErrorVisible[index] && (
                        <span className="error">
                          {t("dlg.titlesValueRequired")}
                        </span>
                      )}
                      {rowDuplicateErrorVisible[index] && (
                        <span className="error">
                          {t("dlg.titlesLocaleDuplicate")}
                        </span>
                      )}
                    </div>
                  )}
                </div>
              ))}
            </div>
            <button type="button" className="secondary" onClick={addTitleRow}>
              {t("dlg.titlesAdd")}
            </button>
            <div className="dlg-hint">{t("dlg.titlesHint")}</div>
          </div>

          <div
            className={`config-field${nsErrorVisible || fieldError === "namespace" ? " has-error" : ""}`}
          >
            <label htmlFor="dlg-namespace">{t("dlg.nsLabel")}</label>
            <input
              id="dlg-namespace"
              type="text"
              value={namespace}
              placeholder="my-namespace"
              onChange={(e) => setNamespace(e.target.value)}
              onBlur={() => markTouched("namespace")}
            />
            <div className={`dlg-hint${nsErrorVisible ? " error" : ""}`}>
              {t("dlg.nsHint")}
            </div>
          </div>

          <div className="config-field">
            <label>{t("dlg.shapeLabel")}</label>
            <label className="checkbox-line">
              <input
                type="checkbox"
                checked={entry}
                onChange={(e) => setEntry(e.target.checked)}
              />
              {t("dlg.shapeEntry")}
            </label>
            <label className="checkbox-line">
              <input
                type="checkbox"
                checked={launch}
                onChange={(e) => setLaunch(e.target.checked)}
              />
              {t("dlg.shapeLaunch")}
            </label>
            {launch && (
              <>
                <div
                  className="dlg-template-group"
                  role="radiogroup"
                  aria-label={t("dlg.templateLabel")}
                >
                  {(["cargo", "deno", "bun"] as const).map((option) => (
                    <label key={option} className="checkbox-line">
                      <input
                        type="radio"
                        name="dlg-backend-template"
                        checked={template === option}
                        onChange={() => setTemplate(option)}
                      />
                      {t(
                        `dlg.template${option[0].toUpperCase()}${option.slice(1)}` as ShellKey,
                      )}
                    </label>
                  ))}
                </div>
                <div className="dlg-hint">{t("dlg.templateHint")}</div>
              </>
            )}
            <div className={`dlg-hint${shape === null ? " error" : ""}`}>
              {t(shapeKey)}
            </div>
          </div>

          <div
            className={`config-field${targetDirErrorVisible ? " has-error" : ""}`}
          >
            <label htmlFor="dlg-target-dir">
              <span className="req">*</span>
              {t("dlg.targetDirLabel")}
            </label>
            {/* Composite path control: the input stays freely typable, the
              embedded trailing button opens the OS directory picker and
              fills the path (same composite form language as the locale
              rows) */}
            <div
              className={`dlg-path-combo${targetDirErrorVisible ? " has-error" : ""}`}
            >
              <input
                id="dlg-target-dir"
                type="text"
                value={targetDir}
                placeholder={t("dlg.targetDirPlaceholder")}
                onChange={(e) => setTargetDir(e.target.value)}
                onBlur={() => markTouched("targetDir")}
              />
              <button
                type="button"
                className="dlg-path-browse"
                title={t("dlg.browse")}
                aria-label={t("dlg.browse")}
                // Suppress the mousedown focus shift: browsing IS filling this
                // field — the input must not blur (no premature required-error,
                // and no re-render between mousedown and click swallowing the
                // first click — the dialog only opened on the second attempt
                // without this)
                onMouseDown={(e) => e.preventDefault()}
                onClick={async () => {
                  const picked = await open({ directory: true });
                  if (typeof picked === "string") setTargetDir(picked);
                }}
              >
                …
              </button>
            </div>
            {targetDirErrorVisible && (
              <div className="dlg-hint error">{t("dlg.targetDirRequired")}</div>
            )}
            <div className="dlg-hint">{t("dlg.targetDirHint")}</div>
          </div>

          <details
            className="dlg-advanced"
            onToggle={(e) => {
              // Expanding while scrolled near the bottom reveals content below
              // the fold — bring the revealed region into view (nearest: no
              // scroll when already visible, minimal travel otherwise).
              // currentTarget is only valid during synchronous dispatch, so
              // capture it before the async frame
              const details = e.currentTarget as HTMLDetailsElement;
              if (details.open) {
                requestAnimationFrame(() => {
                  details
                    .querySelector(".dlg-advanced-body")
                    ?.scrollIntoView({ behavior: "smooth", block: "nearest" });
                });
              }
            }}
          >
            <summary>{t("dlg.advanced")}</summary>
            <div className="dlg-advanced-body">
              <div
                className={`config-field${portErrorVisible || fieldError === "devPort" ? " has-error" : ""}`}
              >
                <label htmlFor="dlg-dev-port">{t("dlg.devPortLabel")}</label>
                <input
                  id="dlg-dev-port"
                  type="number"
                  min={1}
                  max={65535}
                  value={devPort}
                  onChange={(e) => setDevPort(e.target.value)}
                  onBlur={() => markTouched("devPort")}
                />
                {portErrorVisible && (
                  <div className="dlg-hint error">{t("dlg.portInvalid")}</div>
                )}
              </div>

              <div className="config-field">
                <label>{t("dlg.sdkModeLabel")}</label>
                <label className="checkbox-line">
                  <input
                    type="radio"
                    name="dlg-sdk-mode"
                    checked={sdkMode === "registry"}
                    onChange={() => setSdkMode("registry")}
                  />
                  {t("dlg.sdkRegistry")}
                </label>
                <label className="checkbox-line">
                  <input
                    type="radio"
                    name="dlg-sdk-mode"
                    checked={sdkMode === "local"}
                    onChange={() => setSdkMode("local")}
                  />
                  {t("dlg.sdkLocal")}
                </label>
              </div>

              {sdkMode === "local" && (
                <div
                  className={`config-field${sdkPathErrorVisible || fieldError === "sdkPath" ? " has-error" : ""}`}
                >
                  <label htmlFor="dlg-sdk-path">
                    <span className="req">*</span>
                    {t("dlg.sdkPathLabel")}
                  </label>
                  <input
                    id="dlg-sdk-path"
                    type="text"
                    value={sdkPath}
                    placeholder={t("dlg.sdkPathPlaceholder")}
                    onChange={(e) => setSdkPath(e.target.value)}
                    onBlur={() => markTouched("sdkPath")}
                  />
                  {sdkPathErrorVisible && (
                    <div className="dlg-hint error">
                      {t("dlg.sdkPathRequired")}
                    </div>
                  )}
                </div>
              )}
            </div>
          </details>
        </div>
      )}
    </Modal>
  );
}
