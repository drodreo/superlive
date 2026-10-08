// Lightweight modal layer: a fixed overlay + centered card, zero dependencies (the shell DOM has no transform container,
// so fixed positions directly relative to the viewport). Title/body/footer actions are composed by the caller.

import type { ReactNode } from 'react';

export function Modal({
  title,
  children,
  footer,
}: {
  title: string;
  children: ReactNode;
  footer?: ReactNode;
}) {
  return (
    <div className="modal-backdrop" role="dialog" aria-modal="true" aria-label={title}>
      <div className="modal">
        <div className="modal-title">{title}</div>
        <div className="modal-body">{children}</div>
        {footer && <div className="modal-footer">{footer}</div>}
      </div>
    </div>
  );
}
