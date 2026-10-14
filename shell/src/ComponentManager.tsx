// Component management page: installed component list + install (drag a zip
// / enter a path) + four-state badges
// + start/stop/uninstall + configuration-plane wiring.
// Install channel note: for security the webview's input[type=file] cannot
// get disk paths (content only), while the install command's contract is a
// zip disk path — hence the dual channel of window drag-and-drop (tauri core
// API, the event carries the real path, zero new dependencies) + manual path
// entry as the fallback.
// Lifecycle wiring (architecture.md "Configuration plane"): install complete
// → "Start now" prompt → needs-config routed to the config page (the start
// gate) / the rest go to the launch options dialog → start;
// after an upgrade completes, display follows the component state naturally
// (needs-config can fall back from any state).

import { useCallback, useEffect, useRef, useState } from 'react';
import { firstValueFrom, type Observable } from 'rxjs';
import { getCurrentWebview } from '@tauri-apps/api/webview';
import { ask } from '@tauri-apps/plugin-dialog';
import { useTranslation } from 'react-i18next';
import {
  getComponentState,
  installComponent,
  listComponents,
  startComponent,
  stopComponent,
  uninstallComponent,
  type ComponentInfo,
  type ComponentState,
} from './api';
import { navigate } from './router';
import { Modal } from './Modal';
import { LaunchOptionsDialog } from './LaunchOptionsDialog';
import { CreateComponentDialog } from './CreateComponentDialog';
import { componentMenuTitle } from './i18n';
import type { CreateComponentResult } from './api';
import type { ShellKey } from './i18n';

/** Message-bar payload: key + params (not a finished string) so an already-shown message retranslates on locale switch. */
type I18nMessage = { key: ShellKey; params?: Record<string, string | number> };

/**
 * Localized display title of a component (architecture.md "Localized titles"):
 * components with a menu render through the menuTitle chain (the namespace
 * resource the shell injected from the manifest's titles); menu-less
 * components keep the assembly-time derived display_name.
 */
function displayTitleOf(c: ComponentInfo): string {
  // ?? and || may not be mixed unparenthesized — the ?? chain resolves first,
  // then the || id fallback
  return ((c.menu ? componentMenuTitle(c.namespace) : null) ?? c.display_name) || c.id;
}

export function ComponentManager({ onChanged }: { onChanged?: () => void }) {
  const { t } = useTranslation();
  const [components, setComponents] = useState<ComponentInfo[]>([]);
  // Four-state map (merged per component via get_component_state); missing
  // entries fall back to rendering with the running boolean
  const [states, setStates] = useState<Record<string, ComponentState>>({});
  const [message, setMessage] = useState<I18nMessage | null>(null);
  const [busy, setBusy] = useState(false);
  const [zipPath, setZipPath] = useState('');
  // Target component id for the post-install "Start now" prompt / the
  // launch options dialog
  const [installPromptId, setInstallPromptId] = useState<string | null>(null);
  const [launchDialogId, setLaunchDialogId] = useState<string | null>(null);
  // Built-in creation flow: the create dialog's open state + the succeeded
  // result (the post-create guide modal's content)
  const [createOpen, setCreateOpen] = useState(false);
  const [created, setCreated] = useState<CreateComponentResult | null>(null);
  // ref mirror of busy: lets the drag listener (a stable closure mounted once) gate too
  const busyRef = useRef(false);

  const refresh = useCallback(() => {
    const sub = listComponents().subscribe({
      next: (list) => {
        setComponents(list);
        // Four-state merge: get_component_state fetched concurrently per
        // component (single-digit component counts make this acceptable);
        // components whose fetch fails are not written to the map, and the
        // badge falls back to ComponentInfo.running.
        void Promise.all(
          list.map((c) =>
            firstValueFrom(getComponentState(c.id))
              .then((s) => [c.id, s] as const)
              .catch(() => null),
          ),
        ).then((rows) => {
          const next: Record<string, ComponentState> = {};
          for (const row of rows) if (row) next[row[0]] = row[1];
          setStates(next);
        });
      },
      error: (e) => setMessage({ key: 'mgr.listFailed', params: { error: String(e) } }),
    });
    return () => sub.unsubscribe();
  }, []);

  useEffect(() => refresh(), [refresh]);

  // Window drag-and-drop install: the drop event's paths are real disk
  // paths, matching the install contract directly
  useEffect(() => {
    const promise = getCurrentWebview().onDragDropEvent((event) => {
      if (event.payload.type !== 'drop') return;
      const zip = event.payload.paths.find((p) => p.endsWith('.zip'));
      if (!zip) return;
      void run(installComponent(zip), 'mgr.installedDrag', { path: zip }).then((info) => {
        onChanged?.();
        // Install success → lifecycle wiring step one: show the "Start now" prompt
        if (info) setInstallPromptId(info.id);
      });
    });
    return () => {
      promise.then((unlisten) => unlisten());
    };
    // The drag listener is mounted once; run is a ref-gated stable closure
    // and needs no dependency entry
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  /** Unified action executor: busy gating (a ref, safe to reuse from non-React callbacks) + feedback + returns the result value. */
  const run = useCallback(
    async <T,>(
      action$: Observable<T>,
      successKey: ShellKey,
      successParams?: Record<string, string | number>,
    ): Promise<T | undefined> => {
      if (busyRef.current) return undefined;
      busyRef.current = true;
      setBusy(true);
      setMessage(null);
      try {
        const value = await firstValueFrom(action$);
        setMessage({ key: successKey, params: successParams });
        return value;
      } catch (e) {
        setMessage({ key: 'mgr.actionFailed', params: { error: String(e) } });
        return undefined;
      } finally {
        busyRef.current = false;
        setBusy(false);
      }
    },
    [],
  );

  /** Post-change wrap-up: notify the parent (menu refresh) + re-fetch the list (the Promise Observable cleans itself up on completion). */
  const afterChange = useCallback(() => {
    onChanged?.();
    refresh();
  }, [onChanged, refresh]);

  const installByPath = () => {
    const path = zipPath.trim();
    if (!path) return;
    void run(installComponent(path), 'mgr.installed', { path }).then((info) => {
      setZipPath('');
      afterChange();
      if (info) setInstallPromptId(info.id);
    });
  };

  /** The component's current four-state: states hit first; missing entries fall back to the running boolean. */
  const stateOf = (c: ComponentInfo): ComponentState =>
    states[c.id] ?? (c.running ? 'running' : 'stopped');

  // Post-install "Start now": the start gate per the lifecycle contract —
  // needs-config is routed to the config page,
  // the rest (zero-config ready / stopped) go to the launch options dialog
  // (the config gate is naturally transparent to zero-config components).
  const beginLaunchFlow = async (id: string) => {
    setInstallPromptId(null);
    try {
      const state = await firstValueFrom(getComponentState(id));
      if (state === 'needs-config') {
        navigate(`/config/${id}`);
      } else {
        setLaunchDialogId(id);
      }
    } catch (e) {
      setMessage({ key: 'mgr.stateReadFailed', params: { error: String(e) } });
    }
  };

  // Launch options confirmed (policy saved) → start the component → refresh the list
  const launchWithOptionsDone = async (id: string) => {
    setLaunchDialogId(null);
    try {
      await firstValueFrom(startComponent(id));
      setMessage({ key: 'mgr.started', params: { id } });
    } catch (e) {
      setMessage({ key: 'mgr.startFailed', params: { error: String(e) } });
    }
    afterChange();
  };

  return (
    <section className="manager">
      <div className="install-bar">
        <input
          type="text"
          placeholder={t('mgr.installPlaceholder')}
          value={zipPath}
          onChange={(e) => setZipPath(e.target.value)}
        />
        <button type="button" disabled={busy || !zipPath.trim()} onClick={installByPath}>
          {t('mgr.install')}
        </button>
        <button type="button" className="secondary" onClick={() => setCreateOpen(true)}>
          {t('mgr.create')}
        </button>
      </div>

      {message && <div className="message">{t(message.key, message.params)}</div>}

      <table className="comp-table">
        <thead>
          <tr>
            <th>{t('table.component')}</th>
            <th>{t('table.version')}</th>
            <th>{t('table.namespace')}</th>
            <th>{t('table.ui')}</th>
            <th>{t('table.state')}</th>
            <th>{t('table.actions')}</th>
          </tr>
        </thead>
        <tbody>
          {components.length === 0 && (
            <tr>
              <td colSpan={6} className="empty">
                {t('mgr.empty')}
              </td>
            </tr>
          )}
          {components.map((c) => {
            const state = stateOf(c);
            return (
              <tr key={c.id}>
                <td>
                  <div className="comp-name">
                    {displayTitleOf(c)}
                    {/* dev = created in place, body stays in the project dir (vs installed packages) */}
                    {c.source === 'dev' && <span className="badge badge-dev">dev</span>}
                  </div>
                  {c.project_dir && (
                    <div className="comp-project-dir" title={c.project_dir}>
                      {c.project_dir}
                    </div>
                  )}
                </td>
                <td>{c.version}</td>
                <td>{c.namespace}</td>
                <td>{c.ui?.ui_type}</td>
                <td>
                  <span className={`badge badge-${state}`}>{t(`state.${state}`)}</span>
                </td>
                <td>
                  {state === 'needs-config' ? (
                    // Start gate: no starting with required config missing; guide to the config page
                    <button
                      type="button"
                      disabled={busy}
                      onClick={() => navigate(`/config/${c.id}`)}
                    >
                      {t('mgr.goConfig')}
                    </button>
                  ) : (
                    <>
                      {/* Management entry: ready/running/stopped components can all enter the config page to edit config (after saving, effectiveness is routed by state) */}
                      <button
                        type="button"
                        disabled={busy}
                        onClick={() => navigate(`/config/${c.id}`)}
                      >
                        {t('mgr.configure')}
                      </button>{' '}
                      {state === 'running' ? (
                        <button
                          type="button"
                          disabled={busy}
                          onClick={() =>
                            void run(stopComponent(c.id), 'mgr.stopRequested', { id: c.id }).then(
                              afterChange,
                            )
                          }
                        >
                          {t('mgr.stop')}
                        </button>
                      ) : (
                        // ready / stopped: manual start
                        <button
                          type="button"
                          disabled={busy}
                          onClick={() =>
                            void run(startComponent(c.id), 'mgr.started', { id: c.id }).then(
                              afterChange,
                            )
                          }
                        >
                          {t('mgr.start')}
                        </button>
                      )}
                    </>
                  )}
                  <button
                    type="button"
                    className="danger"
                    disabled={busy}
                    onClick={async () => {
                      const confirmKey =
                        c.source === 'dev' ? 'mgr.uninstallConfirmDev' : 'mgr.uninstallConfirm';
                      // window.confirm is not reliable in the tauri webview
                      // (it shows a GTK dialog but returns immediately without
                      // blocking — the uninstall used to run before the user
                      // answered, and Cancel could not stop it). The dialog
                      // plugin's ask() is a proper awaited promise.
                      const ok = await ask(t(confirmKey, { id: c.id }));
                      if (!ok) return;
                      void run(uninstallComponent(c.id), 'mgr.uninstalled', { id: c.id }).then(
                        afterChange,
                      );
                    }}
                  >
                    {t('mgr.uninstall')}
                  </button>
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>

      {/* Install complete → "Start now" prompt (lifecycle wiring step one) */}
      {installPromptId && (
        <Modal
          title={t('mgr.installedTitle')}
          footer={
            <>
              <button type="button" disabled={busy} onClick={() => setInstallPromptId(null)}>
                {t('mgr.later')}
              </button>
              <button
                type="button"
                className="primary"
                disabled={busy}
                onClick={() => void beginLaunchFlow(installPromptId)}
              >
                {t('mgr.startNow')}
              </button>
            </>
          }
        >
          <p>
            {t('mgr.installedPrompt', {
              // Fallback surface: display_name is the assembly-time product of
              // the titles chain (zh → en → first row) — contract-compliant
              name:
                components.find((c) => c.id === installPromptId)?.display_name ?? installPromptId,
            })}
          </p>
        </Modal>
      )}

      {/* Launch options dialog (the prompt step after config completes / for zero-config; an extensible slot) */}
      {launchDialogId && (
        <LaunchOptionsDialog
          id={launchDialogId}
          onDone={() => void launchWithOptionsDone(launchDialogId)}
          onCancel={() => setLaunchDialogId(null)}
        />
      )}

      {/* Built-in creation flow: the create dialog; on success it swaps to the guide modal */}
      {createOpen && (
        <CreateComponentDialog
          onCreated={(result) => {
            setCreateOpen(false);
            setCreated(result);
            afterChange();
          }}
          onCancel={() => setCreateOpen(false)}
        />
      )}

      {/* Post-create guide: project location + the scaffold's next steps (the
          backend/frontend commands are already shape-trimmed by the scaffold) */}
      {created && (
        <Modal
          title={t('done.title')}
          footer={
            <button type="button" className="primary" onClick={() => setCreated(null)}>
              {t('done.close')}
            </button>
          }
        >
          <p>{t('done.intro', { name: created.component.display_name || created.component.id })}</p>
          <div className="done-section">
            <div className="done-label">{t('done.projectDir')}</div>
            <div className="done-path">{created.project_dir}</div>
          </div>
          <div className="done-section">
            <div className="done-label">{t('done.nextSteps')}</div>
            <ol className="done-steps">
              {created.next_steps.map((step) => (
                <li key={step}>
                  <code>{step}</code>
                </li>
              ))}
            </ol>
          </div>
          <p className="done-note">{t('done.locationNote')}</p>
        </Modal>
      )}
    </section>
  );
}
