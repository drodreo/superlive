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

const STATE_LABEL: Record<ComponentState, string> = {
  'needs-config': '待配置',
  ready: '就绪',
  running: '运行中',
  stopped: '已停止',
};

export function ComponentManager({ onChanged }: { onChanged?: () => void }) {
  const [components, setComponents] = useState<ComponentInfo[]>([]);
  // Four-state map (merged per component via get_component_state); missing
  // entries fall back to rendering with the running boolean
  const [states, setStates] = useState<Record<string, ComponentState>>({});
  const [message, setMessage] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [zipPath, setZipPath] = useState('');
  // Target component id for the post-install "Start now" prompt / the
  // launch options dialog
  const [installPromptId, setInstallPromptId] = useState<string | null>(null);
  const [launchDialogId, setLaunchDialogId] = useState<string | null>(null);
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
      error: (e) => setMessage(`读取组件列表失败: ${e}`),
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
      void run(installComponent(zip), `已安装（拖拽）: ${zip}`).then((info) => {
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
    async <T,>(action$: Observable<T>, success: string): Promise<T | undefined> => {
      if (busyRef.current) return undefined;
      busyRef.current = true;
      setBusy(true);
      setMessage(null);
      try {
        const value = await firstValueFrom(action$);
        setMessage(success);
        return value;
      } catch (e) {
        setMessage(`操作失败: ${e}`);
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
    void run(installComponent(path), `已安装: ${path}`).then((info) => {
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
      setMessage(`读取组件状态失败: ${e}`);
    }
  };

  // Launch options confirmed (policy saved) → start the component → refresh the list
  const launchWithOptionsDone = async (id: string) => {
    setLaunchDialogId(null);
    try {
      await firstValueFrom(startComponent(id));
      setMessage(`已启动: ${id}`);
    } catch (e) {
      setMessage(`启动失败: ${e}`);
    }
    afterChange();
  };

  return (
    <section className="manager">
      <h2>组件管理</h2>

      <div className="install-bar">
        <input
          type="text"
          placeholder="组件包 zip 的磁盘路径（也可把 zip 拖进窗口安装）"
          value={zipPath}
          onChange={(e) => setZipPath(e.target.value)}
        />
        <button type="button" disabled={busy || !zipPath.trim()} onClick={installByPath}>
          安装
        </button>
      </div>

      {message && <div className="message">{message}</div>}

      <table className="comp-table">
        <thead>
          <tr>
            <th>组件</th>
            <th>版本</th>
            <th>命名空间</th>
            <th>UI</th>
            <th>状态</th>
            <th>操作</th>
          </tr>
        </thead>
        <tbody>
          {components.length === 0 && (
            <tr>
              <td colSpan={6} className="empty">
                尚未安装任何组件
              </td>
            </tr>
          )}
          {components.map((c) => {
            const state = stateOf(c);
            return (
              <tr key={c.id}>
                <td>{c.display_name || c.id}</td>
                <td>{c.version}</td>
                <td>{c.namespace}</td>
                <td>{c.ui.ui_type}</td>
                <td>
                  <span className={`badge badge-${state}`}>{STATE_LABEL[state]}</span>
                </td>
                <td>
                  {state === 'needs-config' ? (
                    // Start gate: no starting with required config missing; guide to the config page
                    <button
                      type="button"
                      disabled={busy}
                      onClick={() => navigate(`/config/${c.id}`)}
                    >
                      去配置
                    </button>
                  ) : (
                    <>
                      {/* Management entry: ready/running/stopped components can all enter the config page to edit config (after saving, effectiveness is routed by state) */}
                      <button
                        type="button"
                        disabled={busy}
                        onClick={() => navigate(`/config/${c.id}`)}
                      >
                        配置
                      </button>{' '}
                      {state === 'running' ? (
                        <button
                          type="button"
                          disabled={busy}
                          onClick={() =>
                            void run(stopComponent(c.id), `已请求停止: ${c.id}`).then(afterChange)
                          }
                        >
                          停止
                        </button>
                      ) : (
                        // ready / stopped: manual start
                        <button
                          type="button"
                          disabled={busy}
                          onClick={() =>
                            void run(startComponent(c.id), `已启动: ${c.id}`).then(afterChange)
                          }
                        >
                          启动
                        </button>
                      )}
                    </>
                  )}
                  <button
                    type="button"
                    className="danger"
                    disabled={busy}
                    onClick={() => {
                      if (!window.confirm(`卸载组件 ${c.id}？（将停止进程并删除全部数据）`)) return;
                      void run(uninstallComponent(c.id), `已卸载: ${c.id}`).then(afterChange);
                    }}
                  >
                    卸载
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
          title="组件已安装"
          footer={
            <>
              <button type="button" disabled={busy} onClick={() => setInstallPromptId(null)}>
                稍后
              </button>
              <button
                type="button"
                className="primary"
                disabled={busy}
                onClick={() => void beginLaunchFlow(installPromptId)}
              >
                立即启动
              </button>
            </>
          }
        >
          <p>
            组件「
            {components.find((c) => c.id === installPromptId)?.display_name ?? installPromptId}
            」安装完成。是否立即启动？存在未完成必填配置的组件会先进入配置页。
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
    </section>
  );
}
