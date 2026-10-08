/**
 * FormDialog - form dialog component (the superlive privatized edition, copy hardcoded in Chinese).
 */
import type { ReactNode } from 'react';
import { Modal } from './Modal';
import { Button } from './Button';

export interface FormDialogProps {
  open: boolean;
  onClose: () => void;
  onSubmit: () => void;
  title: string;
  children: ReactNode;
  submitText?: string;
  cancelText?: string;
  loading?: boolean;
  submitDisabled?: boolean;
}

export function FormDialog({
  open,
  onClose,
  onSubmit,
  title,
  children,
  submitText = '保存',
  cancelText = '取消',
  loading,
  submitDisabled,
}: FormDialogProps) {
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
            variant="primary"
            size="md"
            onClick={onSubmit}
            disabled={loading || submitDisabled}
          >
            {loading ? '保存中…' : submitText}
          </Button>
        </>
      }
    >
      <div className="sl-ui-form-dialog-body">{children}</div>
    </Modal>
  );
}

export default FormDialog;
