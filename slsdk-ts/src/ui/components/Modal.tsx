/**
 * Modal - modal component (React Portal rendering to body).
 *
 * Portal safety (architecture.md CSS contract): shared component class names are the namespace (.sl-ui-*)
 * — styles are low-specificity single-class selectors, so portal nodes mounted on document.body (outside the component tree)
 * still match; no componentization-era adaptation "wrapping in a domain app container class" is needed (that adaptation was a relic of
 * the domain-container descendant-selector era, dissolved in the facility edition).
 */
import type { ReactNode } from 'react';
import { useEffect, useId } from 'react';
import { createPortal } from 'react-dom';

export interface ModalProps {
  open: boolean;
  onClose: () => void;
  title: string;
  children: ReactNode;
  footer?: ReactNode;
}

export function Modal({ open, onClose, title, children, footer }: ModalProps) {
  // Unique id: avoids fixed-id collisions when multiple Modals are on screen (used for aria association)
  const titleId = useId();

  // Close on ESC
  useEffect(() => {
    if (!open) return;
    function handleKeyDown(e: KeyboardEvent) {
      if (e.key === 'Escape') onClose();
    }
    document.addEventListener('keydown', handleKeyDown);
    return () => document.removeEventListener('keydown', handleKeyDown);
  }, [open, onClose]);

  // Lock scrolling while open
  useEffect(() => {
    if (open) {
      document.body.style.overflow = 'hidden';
    } else {
      document.body.style.overflow = '';
    }
    return () => { document.body.style.overflow = ''; };
  }, [open]);

  if (!open) return null;

  return createPortal(
    <div
      className="sl-ui-modal-backdrop"
      onClick={(e) => { if (e.target === e.currentTarget) onClose(); }}
      role="dialog"
      aria-modal="true"
      aria-labelledby={titleId}
    >
      <div className="sl-ui-modal-container">
        <div className="sl-ui-modal-header">
          <h2 id={titleId} className="sl-ui-modal-title">{title}</h2>
          <button className="sl-ui-modal-close" onClick={onClose} aria-label="关闭">
            ✕
          </button>
        </div>
        <div className="sl-ui-modal-body">{children}</div>
        {footer && <div className="sl-ui-modal-footer">{footer}</div>}
      </div>
    </div>,
    document.body
  );
}

export default Modal;
