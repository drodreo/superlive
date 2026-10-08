// Launch options dialog (architecture.md "Launch options slot"):
// Timing = after configuration completes and validation passes — the shell's
// "launch options" prompt step; this step is an extensible slot — the current
// first item = launch policy (auto/manual), and future launch options append
// their option sections inside this component (independent state), submitted
// in order via confirm; the parent then runs start_component,
// leaving the lifecycle main flow untouched (install → config gate → launch
// options → start).

import { useState } from 'react';
import { firstValueFrom } from 'rxjs';
import { setLaunchPolicy, type LaunchPolicy } from './api';
import { Modal } from './Modal';

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
  // --- Slot 1: launch policy (component-level, persisted in the shell-side registry, not in the component manifest) ---
  const [policy, setPolicy] = useState<LaunchPolicy>('auto');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Confirm = run each slot option's command call in order (currently only
  // the launch policy)
  const confirm = async () => {
    setBusy(true);
    setError(null);
    try {
      await firstValueFrom(setLaunchPolicy(id, policy));
      onDone();
    } catch (e) {
      setError(`保存启动策略失败: ${e}`);
    } finally {
      setBusy(false);
    }
  };

  return (
    <Modal
      title="启动选项"
      footer={
        <>
          <button type="button" disabled={busy} onClick={onCancel}>
            稍后再说
          </button>
          <button type="button" className="primary" disabled={busy} onClick={() => void confirm()}>
            保存并启动
          </button>
        </>
      }
    >
      {error && <div className="message error">{error}</div>}
      <div className="launch-option">
        <div className="launch-option-title">启动策略</div>
        <label>
          <input
            type="radio"
            name={`launch-policy-${id}`}
            checked={policy === 'auto'}
            onChange={() => setPolicy('auto')}
          />
          自动：每次打开 superlive 时自动拉起该组件
        </label>
        <label>
          <input
            type="radio"
            name={`launch-policy-${id}`}
            checked={policy === 'manual'}
            onChange={() => setPolicy('manual')}
          />
          手动：在组件管理页手动启动
        </label>
      </div>
      {/* Future launch-options slots: append option sections here (independent state, submitted in order via confirm) */}
    </Modal>
  );
}
