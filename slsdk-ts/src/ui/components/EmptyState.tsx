/**
 * EmptyState - empty-state component (simple SVG shapes, no external icon library). Verbatim from superlive.
 */
import type { ReactElement, ReactNode } from 'react';

export type EmptyStateIcon = 'search' | 'data' | 'file';

export interface EmptyStateProps {
  icon?: EmptyStateIcon;
  title: string;
  description?: string;
  action?: ReactNode;
}

function SearchEmptyIcon() {
  return (
    <svg className="sl-ui-empty-icon" viewBox="0 0 64 64" fill="none" xmlns="http://www.w3.org/2000/svg">
      <circle cx="26" cy="26" r="18" stroke="currentColor" strokeWidth="3" />
      <line x1="39" y1="39" x2="54" y2="54" stroke="currentColor" strokeWidth="3" strokeLinecap="round" />
      <line x1="18" y1="26" x2="34" y2="26" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" />
    </svg>
  );
}

function DataEmptyIcon() {
  return (
    <svg className="sl-ui-empty-icon" viewBox="0 0 64 64" fill="none" xmlns="http://www.w3.org/2000/svg">
      <rect x="8" y="16" width="48" height="36" rx="4" stroke="currentColor" strokeWidth="3" />
      <line x1="8" y1="28" x2="56" y2="28" stroke="currentColor" strokeWidth="2" />
      <line x1="24" y1="28" x2="24" y2="52" stroke="currentColor" strokeWidth="2" />
      <circle cx="32" cy="40" r="6" stroke="currentColor" strokeWidth="2.5" strokeDasharray="3 3" />
      <line x1="40" y1="16" x2="40" y2="28" stroke="currentColor" strokeWidth="3" />
      <line x1="32" y1="4" x2="32" y2="16" stroke="currentColor" strokeWidth="3" />
    </svg>
  );
}

function FileEmptyIcon() {
  return (
    <svg className="sl-ui-empty-icon" viewBox="0 0 64 64" fill="none" xmlns="http://www.w3.org/2000/svg">
      <path d="M12 8h28l16 16v36a4 4 0 01-4 4H12a4 4 0 01-4-4V12a4 4 0 014-4z" stroke="currentColor" strokeWidth="3" />
      <path d="M40 8v16h16" stroke="currentColor" strokeWidth="3" strokeLinejoin="round" />
      <line x1="22" y1="36" x2="42" y2="36" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" />
      <line x1="22" y1="44" x2="38" y2="44" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" />
    </svg>
  );
}

const icons: Record<EmptyStateIcon, () => ReactElement> = {
  search: SearchEmptyIcon,
  data: DataEmptyIcon,
  file: FileEmptyIcon,
};

export function EmptyState({ icon = 'data', title, description, action }: EmptyStateProps) {
  const Icon = icons[icon];
  return (
    <div className="sl-ui-empty-state">
      <Icon />
      <h3 className="sl-ui-empty-title">{title}</h3>
      {description && <p className="sl-ui-empty-description">{description}</p>}
      {action && <div>{action}</div>}
    </div>
  );
}

export default EmptyState;
