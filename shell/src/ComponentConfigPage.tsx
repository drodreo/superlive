// Component config page (architecture.md "Configuration plane"): renders the
// form dynamically from the manifest config declaration, so users never hand-
// edit config.toml. Two lifecycle channels:
// ① Start gate — entered from the post-first-install/upgrade required-missing
//    (needs-config) or config-complete (ready) state; after a successful save
//    (write_component_config passing shell-side validation) the "launch
//    options" prompt runs before starting;
// ② Management entry — running/stopped components come from the management
//    page to edit config: for running, a save hot-reloads immediately (the
//    shell restarts automatically when a restart_required entry changes); for
//    stopped, a save waits for the next manual start.
// Validation authority lives on the shell Rust side: this page does no
// required/format pre-validation; a failed write's errors map is displayed
// field by field in red (the only exception: NaN for number, a mechanical
// type-conversion check the frontend flags directly).

import { useCallback, useEffect, useState } from 'react';
import { firstValueFrom } from 'rxjs';
import {
  fetchConfigI18n,
  getComponentConfig,
  getComponentState,
  startComponent,
  writeComponentConfig,
  type ComponentConfig,
  type ComponentState,
  type ConfigField,
} from './api';
import { navigate } from './router';
import { LaunchOptionsDialog } from './LaunchOptionsDialog';

type FormValues = Record<string, string>;

/** Parses the Err content of a failed write_component_config: the contract form is a JSON string {"errors":{key: reason}}. */
function parseWriteErrors(e: unknown): Record<string, string> | null {
  if (typeof e !== 'string') return null;
  try {
    const parsed = JSON.parse(e) as { errors?: Record<string, string> };
    if (parsed.errors && typeof parsed.errors === 'object') return parsed.errors;
  } catch {
    // A non-JSON error string → falls to the generic error message
  }
  return null;
}

export function ComponentConfigPage({ id }: { id: string }) {
  const [config, setConfig] = useState<ComponentConfig | null>(null);
  const [i18nMap, setI18nMap] = useState<Record<string, string> | null>(null);
  // The form's editing state is uniformly string-shaped (checkboxes use
  // 'true'/'false'); values are converted to the declared types on submit
  const [form, setForm] = useState<FormValues>({});
  const [errors, setErrors] = useState<Record<string, string>>({});
  const [globalError, setGlobalError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  // Set of visible sensitive fields (password form + show/hide toggle)
  const [sensitiveVisible, setSensitiveVisible] = useState<Set<string>>(new Set());
  // Save-result message bar (wording routed by the component's lifecycle
  // state, see save())
  const [saveInfo, setSaveInfo] = useState<string | null>(null);
  const [showLaunchOptions, setShowLaunchOptions] = useState(false);

  useEffect(() => {
    const sub = getComponentConfig(id).subscribe({
      next: (cfg) => {
        setConfig(cfg);
        // Form initial values: use values directly when present; booleans take
        // the declared default / fall back to false;
        // everything else is an empty string = unset placeholder (contract:
        // keys with no declared default and no file value are absent from
        // values).
        const init: FormValues = {};
        for (const f of cfg.fields) {
          const v = cfg.values[f.key];
          init[f.key] =
            v !== undefined
              ? String(v)
              : f.type === 'boolean'
                ? String(f.default ?? false)
                : '';
        }
        setForm(init);
        // i18n resources come from the component's ui static space (not
        // dependent on the component ESM loading); on failure silently fall
        // back to the raw text
        void fetchConfigI18n(id, cfg.i18n).then(setI18nMap);
      },
      error: (e) => setGlobalError(`读取组件配置声明失败: ${e}`),
    });
    return () => sub.unsubscribe();
  }, [id]);

  /** i18n lookup: label/group values are themselves i18n keys — translated on hit, falling back to the raw text when missing. */
  const tr = useCallback((text: string) => i18nMap?.[text] ?? text, [i18nMap]);

  const setValue = (key: string, value: string) => setForm((f) => ({ ...f, [key]: value }));

  const toggleVisible = (key: string) =>
    setSensitiveVisible((s) => {
      const next = new Set(s);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });

  // Aggregate groups in declaration order (same-named groups merged; group
  // order = order of first appearance); null groups render no group header
  const grouped: { name: string | null; fields: ConfigField[] }[] = [];
  if (config) {
    for (const f of config.fields) {
      const idx = grouped.findIndex((g) => g.name === f.group);
      if (idx >= 0) grouped[idx].fields.push(f);
      else grouped.push({ name: f.group, fields: [f] });
    }
  }

  const save = async () => {
    if (!config) return;
    setBusy(true);
    setErrors({});
    setGlobalError(null);
    setSaveInfo(null);
    try {
      // NaN for number is a mechanical type-conversion check (the validation
      // authority lives on the shell Rust side; not duplicated here)
      const localErrors: Record<string, string> = {};
      for (const f of config.fields) {
        const raw = form[f.key] ?? '';
        if (f.type === 'number' && raw !== '' && Number.isNaN(Number(raw))) {
          localErrors[f.key] = '须为数字';
        }
      }
      if (Object.keys(localErrors).length > 0) {
        setErrors(localErrors);
        return;
      }

      const payload: Record<string, string | number | boolean> = {};
      for (const f of config.fields) {
        const raw = form[f.key] ?? '';
        if (f.type === 'boolean') {
          payload[f.key] = raw === 'true';
        } else if (raw !== '') {
          payload[f.key] = f.type === 'number' ? Number(raw) : raw;
        } else if (!f.required && config.values[f.key] !== undefined) {
          // Non-required with an existing value: explicitly submitting an empty
          // string = clear semantics (the endpoint writes back an empty string,
          // read back as empty)
          payload[f.key] = '';
        }
        // Other empty strings do not submit the key: required unfilled (the
        // missing signal is left for shell-side validation to block) /
        // non-required with no prior value (submitting an empty string is
        // meaningless; the file stays without a value)
      }
      const result = await firstValueFrom(writeComponentConfig(id, payload));
      if (result.restart_required_changed) {
        // Precondition = the component was running before the save: the shell
        // restarts automatically after writing (contract). It may still be
        // within the restart window, so instead of distinguishing running via
        // get_component_state, the message speaks in restart semantics
        // directly.
        setSaveInfo('已保存。涉及需重启的配置项，组件已由壳自动重启。');
        return;
      }
      // No automatic restart triggered → route by the component's lifecycle
      // state (management entry / gate flow)
      let state: ComponentState;
      try {
        state = await firstValueFrom(getComponentState(id));
      } catch {
        // A state-read failure does not affect the save's success: a neutral
        // message, not entering the gate flow (avoiding disturbing a
        // stopped-state user)
        setSaveInfo('已保存');
        return;
      }
      if (state === 'running') {
        // Management entry: no restart_required entries changed → the
        // component's standard endpoint hot-reloads immediately
        setSaveInfo('已保存，配置已热重载生效。');
      } else if (state === 'stopped') {
        // Management entry: no automatic start; the user starts manually from
        // the management page
        setSaveInfo('已保存，组件未运行。');
      } else {
        // needs-config / ready (the gate flow) → the launch options prompt (an
        // extensible slot, see LaunchOptionsDialog); timing differences in the
        // shell-side state machine's flip after a successful needs-config save
        // do not affect this branch (both states belong to the gate flow).
        setShowLaunchOptions(true);
      }
    } catch (e) {
      const fieldErrors = parseWriteErrors(e);
      if (fieldErrors) setErrors(fieldErrors);
      else setGlobalError(`保存配置失败: ${String(e)}`);
    } finally {
      setBusy(false);
    }
  };

  // After launch options confirmed: start the component and return to the
  // management page; a start failure stays on this page showing the error
  const launchDone = async () => {
    try {
      await firstValueFrom(startComponent(id));
      setShowLaunchOptions(false);
      navigate('/components');
    } catch (e) {
      setShowLaunchOptions(false);
      setGlobalError(`启动失败: ${e}`);
    }
  };

  const renderField = (f: ConfigField) => {
    const raw = form[f.key] ?? '';
    const err = errors[f.key];
    const visible = sensitiveVisible.has(f.key);
    // Masking first: sensitive and not toggled open always renders as a
    // password field (including sensitive number/datetime entries)
    const inputType =
      f.sensitive && !visible
        ? 'password'
        : f.type === 'number'
          ? 'number'
          : f.type === 'datetime'
            ? 'datetime-local'
            : 'text';
    // Reserved description channel: the i18n key = {key}.description (the
    // manifest declaration has no standalone description field)
    const desc = i18nMap?.[`${f.key}.description`];
    return (
      <div className={`config-field${err ? ' has-error' : ''}`} key={f.key}>
        <label htmlFor={`cfg-${f.key}`}>
          {f.required && (
            <span className="req" title="必填">
              *
            </span>
          )}
          {tr(f.label)}
          {f.restart_required && <span className="restart-tag">重启生效</span>}
        </label>
        {f.type === 'select' ? (
          <select id={`cfg-${f.key}`} value={raw} onChange={(e) => setValue(f.key, e.target.value)}>
            <option value="">（未设置）</option>
            {(f.options ?? []).map((o) => (
              <option key={o} value={o}>
                {o}
              </option>
            ))}
          </select>
        ) : f.type === 'boolean' ? (
          <label className="checkbox-line">
            <input
              id={`cfg-${f.key}`}
              type="checkbox"
              checked={raw === 'true'}
              onChange={(e) => setValue(f.key, String(e.target.checked))}
            />
            启用
          </label>
        ) : (
          <div className="sensitive-wrap">
            <input
              id={`cfg-${f.key}`}
              type={inputType}
              value={raw}
              placeholder={
                f.default !== undefined && f.default !== null
                  ? `默认：${String(f.default)}`
                  : '未设置'
              }
              onChange={(e) => setValue(f.key, e.target.value)}
            />
            {f.sensitive && (
              <button type="button" className="link" onClick={() => toggleVisible(f.key)}>
                {visible ? '隐藏' : '显示'}
              </button>
            )}
          </div>
        )}
        {desc && <div className="field-desc">{desc}</div>}
        {err && <div className="field-error">{err}</div>}
      </div>
    );
  };

  if (globalError && !config) {
    return (
      <section className="config-page">
        <div className="comp-error">{globalError}</div>
      </section>
    );
  }
  if (!config) {
    return (
      <section className="config-page">
        <div className="comp-loading">加载配置声明中…</div>
      </section>
    );
  }

  return (
    <section className="config-page">
      <div className="page-head">
        <button type="button" className="link" onClick={() => navigate('/components')}>
          ← 返回组件管理
        </button>
        <h2>配置：{id}</h2>
      </div>

      {globalError && <div className="message error">{globalError}</div>}
      {saveInfo && <div className="message success">{saveInfo}</div>}
      {config.fields.length === 0 && (
        <div className="message">该组件没有可配置项（零配置组件，可直接启动）。</div>
      )}

      {grouped.map((g) => (
        <section className="config-group" key={g.name ?? '__ungrouped__'}>
          {g.name !== null && <h3>{tr(g.name)}</h3>}
          {g.fields.map(renderField)}
        </section>
      ))}

      {config.fields.length > 0 && (
        <div className="config-actions">
          <button type="button" className="primary" disabled={busy} onClick={() => void save()}>
            保存配置
          </button>
        </div>
      )}

      {showLaunchOptions && (
        <LaunchOptionsDialog
          id={id}
          onDone={() => void launchDone()}
          onCancel={() => setShowLaunchOptions(false)}
        />
      )}
    </section>
  );
}
