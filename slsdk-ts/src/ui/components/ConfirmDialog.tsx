/**
 * ConfirmDialog - confirmation dialog component (verbatim from superlive; the default copy was always Chinese).
 * Irreversible operations must display warning text.
 */
import { Modal } from './Modal';
import { Button } from './Button';

export interface ConfirmDialogProps {
  open: boolean;
  onClose: () => void;
  onConfirm: () => void;
  title: string;
  message: string;
  confirmText?: string;
  cancelText?: string;
  danger?: boolean;
  warningText?: string;
  /** Confirm operation in progress (disables both buttons) */
  loading?: boolean;
}

export function ConfirmDialog({
  open,
  onClose,
  onConfirm,
  title,
  message,
  confirmText = '确认',
  cancelText = '取消',
  danger,
  warningText,
  loading,
}: ConfirmDialogProps) {
  return (
    <Modal
      open={open}
      onClose={onClose}
      title={title}
      footer={
        <>
          <Button variant="secondary" size="md" onClick={onClose} disabled={loading}>
            {cancelText}
          </Button>
          <Button
            variant={danger ? 'danger' : 'primary'}
            size="md"
            onClick={() => { onConfirm(); onClose(); }}
            loading={loading}
          >
            {confirmText}
          </Button>
        </>
      }
    >
      <div className="sl-ui-confirm-body">
        <p>{message}</p>
        {warningText && (
          <div className="sl-ui-confirm-warning">{warningText}</div>
        )}
      </div>
    </Modal>
  );
}

export default ConfirmDialog;
